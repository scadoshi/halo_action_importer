use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client as ReqwestClient, StatusCode};
use tracing::{error, warn};

use crate::config::Config;
use crate::domain::models::action_object::ActionObject;
use crate::outbound::client;
use crate::outbound::client::auth::AuthClient;

/// Posts actions to Halo. A post is sent once: Halo may have stored the actions
/// even when the reply is a 504 or never arrives, so sending it again could
/// duplicate them. The one exception is a 401, which gets a new token and one
/// more try.
#[derive(Debug, Clone)]
pub struct ActionClient {
    config: Config,
    http_client: ReqwestClient,
    auth_client: Arc<AuthClient>,
}

impl ActionClient {
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn new(config: Config, auth_client: Arc<AuthClient>) -> anyhow::Result<Self> {
        Self::with_request_timeout(config, auth_client, client::REQUEST_TIMEOUT)
    }

    /// An action client whose posts give up after `timeout`.
    pub(crate) fn with_request_timeout(
        config: Config,
        auth_client: Arc<AuthClient>,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config,
            http_client: client::http_client(timeout)?,
            auth_client,
        })
    }

    /// Posts `action_objects` in one request.
    ///
    /// # Errors
    ///
    /// A [`PostError`] saying why Halo did not take the batch.
    pub async fn post_action_objects(
        &self,
        action_objects: Vec<ActionObject>,
    ) -> Result<(), PostError> {
        if action_objects.is_empty() {
            return Ok(());
        }
        let payload: Vec<_> = action_objects
            .iter()
            .map(|action| action.payload(&self.config.action_id_custom_field))
            .collect();
        let mut endpoint = self.config.base_resource_url.clone();
        endpoint.set_path("api/actions");
        let mut auth_token = self
            .auth_client
            .fetch_token()
            .await
            .map_err(PostError::Auth)?;
        // A 401 gets one new token and one more try; a second 401 is an error.
        let mut refreshed = false;

        loop {
            let response = self
                .http_client
                .post(endpoint.clone())
                .header("Authorization", &auth_token)
                .header("Content-Type", "application/json; charset=utf-8")
                .json(&payload)
                .send()
                .await
                .map_err(PostError::from_send)?;

            let status = response.status();

            if status == StatusCode::UNAUTHORIZED && !refreshed {
                warn!("Received 401 Unauthorized for batch, refreshing token and retrying");
                auth_token = self
                    .auth_client
                    .refresh_token()
                    .await
                    .map_err(PostError::Auth)?;
                refreshed = true;
                continue;
            }

            if status.is_success() {
                return Ok(());
            }

            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to get error response".to_string());
            let error = if status == StatusCode::NOT_FOUND {
                PostError::TicketNotFound { body }
            } else {
                PostError::Rejected { status, body }
            };
            error!("Action object POST failed for batch: {error}");
            return Err(error);
        }
    }
}

/// The error type of an action whose ticket Halo answered 404 for.
pub const TICKET_NOT_FOUND: &str = "Ticket not found";

/// Why Halo did not take a batch of actions.
#[derive(Debug)]
pub enum PostError {
    /// Halo answered 404: the ticket the actions belong to does not exist.
    TicketNotFound { body: String },
    /// Halo answered with any other error status.
    Rejected { status: StatusCode, body: String },
    /// No reply came within the request timeout. Halo may still have stored the batch.
    Timeout(reqwest::Error),
    /// The request could not be sent or its reply could not be read.
    Network(reqwest::Error),
    /// No token could be had to send the request with.
    Auth(anyhow::Error),
}

impl PostError {
    fn from_send(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(error)
        } else {
            Self::Network(error)
        }
    }

    /// The error type `retry.csv` and `summary.json` show for the failed actions.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::TicketNotFound { .. } => TICKET_NOT_FOUND,
            Self::Rejected { status, .. } => match *status {
                StatusCode::BAD_REQUEST => "Validation error",
                StatusCode::UNAUTHORIZED => "Authentication error",
                StatusCode::GATEWAY_TIMEOUT => "Gateway timeout",
                StatusCode::INTERNAL_SERVER_ERROR => "Server error",
                _ => "Unknown error",
            },
            Self::Timeout(_) => "Request timeout",
            Self::Network(_) => "Network error",
            Self::Auth(_) => "Authentication error",
        }
    }
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TicketNotFound { body } => write!(f, "status 404, ticket not found: {body}"),
            Self::Rejected { status, body } => write!(f, "status {status}, error: {body}"),
            Self::Timeout(e) => write!(f, "no reply within the request timeout: {e}"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::Auth(e) => write!(f, "authentication failed: {e:#}"),
        }
    }
}

impl std::error::Error for PostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Timeout(e) | Self::Network(e) => Some(e),
            Self::Auth(e) => Some(e.as_ref()),
            Self::TicketNotFound { .. } | Self::Rejected { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::models::action_object::ActionId;
    use crate::outbound::client::auth::AuthClient;
    use crate::test_server;
    use crate::test_server::{Reply, SHORT_TIMEOUT, TOKEN, TestServer};

    /// How long a test lets a post run before calling it hung.
    const HUNG: std::time::Duration = std::time::Duration::from_secs(5);

    fn client(server: &TestServer) -> ActionClient {
        let config = test_server::config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        ActionClient::new(config, auth_client).unwrap()
    }

    fn action(ticket_id: u32, action_id: u64) -> ActionObject {
        ActionObject::new(
            ticket_id,
            None,
            None,
            "a note",
            "who",
            ActionId::new(action_id),
        )
    }

    const POST_TOKEN: &str = "POST /auth/token HTTP/1.1";
    const POST_ACTIONS: &str = "POST /api/actions HTTP/1.1";

    #[tokio::test]
    async fn a_post_answered_401_fetches_a_new_token_and_posts_again() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(401, "{}"),
            TOKEN,
            Reply::Json(201, "[]"),
        ])
        .await;

        let result = tokio::time::timeout(
            HUNG,
            client(&server).post_action_objects(vec![action(1, 10)]),
        )
        .await
        .expect("the post hung");

        assert!(result.is_ok(), "{result:?}");
        let requests = server.requests();
        assert_eq!(
            test_server::lines(&requests),
            [POST_TOKEN, POST_ACTIONS, POST_TOKEN, POST_ACTIONS]
        );
        assert!(
            requests[1].body.contains(r#""cfactionid":10"#),
            "{}",
            requests[1].body
        );
        assert_eq!(requests[1].body, requests[3].body);
    }

    #[tokio::test]
    async fn a_post_answered_401_twice_is_an_error() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(401, "{}"),
            TOKEN,
            Reply::Json(401, "{}"),
        ])
        .await;

        let result = tokio::time::timeout(
            HUNG,
            client(&server).post_action_objects(vec![action(1, 10)]),
        )
        .await
        .expect("the post hung");

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("401"), "{message}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS, POST_TOKEN, POST_ACTIONS]
        );
    }

    #[tokio::test]
    async fn a_post_answered_504_is_an_error_and_not_sent_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(504, "{}")]).await;

        let result = tokio::time::timeout(
            HUNG,
            client(&server).post_action_objects(vec![action(1, 10)]),
        )
        .await
        .expect("the post hung");

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("504"), "{message}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS]
        );
    }

    #[tokio::test]
    async fn a_post_hit_by_a_network_error_is_an_error_and_not_sent_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Hangup]).await;

        let result = tokio::time::timeout(
            HUNG,
            client(&server).post_action_objects(vec![action(1, 10)]),
        )
        .await
        .expect("the post hung");

        assert!(matches!(result, Err(PostError::Network(_))), "{result:?}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS]
        );
    }

    #[tokio::test]
    async fn a_post_that_times_out_is_an_error_and_not_sent_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Stall]).await;
        let config = test_server::config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        let client =
            ActionClient::with_request_timeout(config, auth_client, SHORT_TIMEOUT).unwrap();

        let result = tokio::time::timeout(HUNG, client.post_action_objects(vec![action(1, 10)]))
            .await
            .expect("the post hung");

        assert!(matches!(result, Err(PostError::Timeout(_))), "{result:?}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS]
        );
    }

    /// Posts one action to a server that answers `status` with `body`.
    async fn post_answered(status: u16, body: &'static str) -> Result<(), PostError> {
        let server = TestServer::start(vec![TOKEN, Reply::Json(status, body)]).await;
        tokio::time::timeout(
            HUNG,
            client(&server).post_action_objects(vec![action(1, 10)]),
        )
        .await
        .expect("the post hung")
    }

    #[tokio::test]
    async fn a_post_answered_404_is_ticket_not_found_whatever_the_body() {
        let result = post_answered(404, "{}").await;
        assert!(
            matches!(result, Err(PostError::TicketNotFound { .. })),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_post_answered_400_is_a_rejection_even_when_the_body_says_not_found() {
        let result = post_answered(400, r#"{"error":"user not found"}"#).await;
        let Err(PostError::Rejected { status, body }) = result else {
            panic!("{result:?}");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("user not found"), "{body}");
    }

    #[test]
    fn error_types_come_from_the_status_or_the_kind_of_failure() {
        let rejected = |status| PostError::Rejected {
            status,
            body: String::new(),
        };
        let cases = [
            (
                PostError::TicketNotFound {
                    body: String::new(),
                },
                "Ticket not found",
            ),
            (rejected(StatusCode::BAD_REQUEST), "Validation error"),
            (rejected(StatusCode::UNAUTHORIZED), "Authentication error"),
            (rejected(StatusCode::GATEWAY_TIMEOUT), "Gateway timeout"),
            (rejected(StatusCode::INTERNAL_SERVER_ERROR), "Server error"),
            (rejected(StatusCode::FORBIDDEN), "Unknown error"),
            (
                PostError::Auth(anyhow::anyhow!("no token")),
                "Authentication error",
            ),
        ];
        for (error, want) in cases {
            assert_eq!(error.error_type(), want, "{error}");
        }
    }

    #[tokio::test]
    #[ignore = "posts a real action to the Halo instance in .env; run with --ignored"]
    async fn post_action_object() {
        let config = Config::from_env().unwrap();
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        let action_client = ActionClient::new(config, auth_client).unwrap();
        let action_object = ActionObject::new(
            2997,
            None,
            None,
            "rusty note",
            "rusty who",
            ActionId::new(897),
        );
        let response = action_client.post_action_objects(vec![action_object]).await;
        assert!(response.is_ok());
    }
}
