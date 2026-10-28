use std::sync::Arc;

use anyhow::Context;
use reqwest::Client as ReqwestClient;
use tracing::{error, warn};

use crate::config::Config;
use crate::domain::models::action_object::ActionObject;
use crate::outbound::client::auth::AuthClient;

#[derive(Debug, Clone)]
pub struct ActionClient {
    config: Config,
    http_client: ReqwestClient,
    auth_client: Arc<AuthClient>,
}

impl ActionClient {
    pub fn new(config: Config, auth_client: Arc<AuthClient>) -> Self {
        Self {
            config,
            http_client: ReqwestClient::new(),
            auth_client,
        }
    }

    pub async fn post_action_objects(
        &self,
        action_objects: Vec<ActionObject>,
    ) -> anyhow::Result<()> {
        if action_objects.is_empty() {
            return Ok(());
        }
        let action_ids: Vec<String> = action_objects
            .iter()
            .map(|a| a.action_id().to_string())
            .collect();
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
            .context("Failed to get valid authentication token")?;
        // A 401 gets one new token and one more try; a second 401 is an error.
        let mut refreshed = false;

        loop {
            let request = self
                .http_client
                .post(endpoint.clone())
                .header("Authorization", &auth_token)
                .header("Content-Type", "application/json; charset=utf-8")
                .json(&payload);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    // Network error, timeout, or connection failure - retry indefinitely
                    warn!(
                        "Network error sending POST request for action IDs {:?}: {} - retrying immediately",
                        action_ids, e
                    );
                    auth_token =
                        self.auth_client.fetch_token().await.context(
                            "Failed to refresh authentication token after network error",
                        )?;
                    continue;
                }
            };

            let status = response.status();

            if status == reqwest::StatusCode::GATEWAY_TIMEOUT {
                warn!(
                    "Received 504 Gateway Timeout for action IDs {:?}, retrying immediately",
                    action_ids
                );
                auth_token = self
                    .auth_client
                    .fetch_token()
                    .await
                    .context("Failed to refresh authentication token after 504")?;
                continue;
            }

            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
                warn!("Received 401 Unauthorized for batch, refreshing token and retrying");
                auth_token = self
                    .auth_client
                    .refresh_token()
                    .await
                    .context("Failed to refresh authentication token after 401")?;
                refreshed = true;
                continue;
            }

            if !status.is_success() {
                let error_text: String = response
                    .text()
                    .await
                    .with_context(|| {
                        format!(
                            "failed to read error response body for action IDs: {:?} (status: {})",
                            action_ids, status
                        )
                    })
                    .unwrap_or_else(|_| "failed to get error response".to_string());
                error!(
                    "Action object POST failed for batch: status {}, error: {}",
                    status, error_text
                );
                anyhow::bail!(
                    "Action object POST failed for batch: status {}, error: {}",
                    status,
                    error_text
                )
            }

            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::models::action_object::ActionId;
    use crate::outbound::client::auth::AuthClient;
    use crate::test_server::{Reply, TOKEN, TestServer, config, lines};

    /// How long a test lets a post run before calling it hung.
    const HUNG: std::time::Duration = std::time::Duration::from_secs(5);

    fn client(server: &TestServer) -> ActionClient {
        let config = config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()));
        ActionClient::new(config, auth_client)
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
            lines(&requests),
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
            lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS, POST_TOKEN, POST_ACTIONS]
        );
    }

    #[tokio::test]
    #[ignore = "posts a real action to the Halo instance in .env; run with --ignored"]
    async fn post_action_object() {
        let config = Config::from_env().unwrap();
        let auth_client = Arc::new(AuthClient::new(config.clone()));
        let action_client = ActionClient::new(config, auth_client);
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
