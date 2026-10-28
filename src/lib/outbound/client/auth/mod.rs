pub mod token;

use std::sync::Arc;

use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::error;

use crate::config::Config;
use crate::outbound::client::auth::token::AuthToken;

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: String,
}

#[derive(Serialize)]
struct TokenRequest {
    client_id: String,
    client_secret: String,
    grant_type: String,
    scope: String,
}

pub struct AuthClient {
    config: Config,
    http_client: ReqwestClient,
    current_token: Arc<Mutex<Option<AuthToken>>>,
}

impl std::fmt::Debug for AuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AuthClient {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            http_client: ReqwestClient::new(),
            current_token: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn get_valid_token(&self) -> anyhow::Result<String> {
        let mut token_guard = self.current_token.lock().await;

        if let Some(token) = token_guard.as_ref()
            && !token.is_expired()
        {
            return Ok(token.header_value());
        }

        let new_token = self.fetch_new_token().await?;
        let header_value = new_token.header_value();
        *token_guard = Some(new_token);

        Ok(header_value)
    }

    async fn fetch_new_token(&self) -> anyhow::Result<AuthToken> {
        let token_request = TokenRequest {
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            grant_type: "client_credentials".to_string(),
            scope: "all".to_string(),
        };

        let response = self
            .http_client
            .post(self.config.token_url.as_str())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&token_request)
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to send authentication request to: {}",
                    self.config.token_url
                )
            })?;

        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            error!(
                "Authentication failed: invalid credentials (status: {})",
                status
            );
            anyhow::bail!("Authentication failed: invalid credentials");
        }

        let response_text = match response.text().await.with_context(|| {
            format!(
                "failed to read authentication response body (status: {})",
                status
            )
        }) {
            Ok(text) => text,
            Err(e) => {
                error!("Failed to read authentication response: {}", e);
                return Err(e);
            }
        };

        if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&response_text) {
            error!(
                "Authentication error: {}: {}",
                error_response.error, error_response.error_description
            );
            anyhow::bail!(
                "Authentication error: {}: {}",
                error_response.error,
                error_response.error_description
            );
        }

        // The body may hold a token, and serde's message can quote values from
        // it, so the error names only the kind of failure and where it is.
        let token_response: TokenResponse = match serde_json::from_str(&response_text) {
            Ok(token) => token,
            Err(e) => {
                let message = format!(
                    "failed to parse token response JSON (status: {}): {:?} error at line {}, column {}",
                    status,
                    e.classify(),
                    e.line(),
                    e.column()
                );
                error!("Failed to parse token response: {}", message);
                anyhow::bail!(message);
            }
        };

        AuthToken::new(
            token_response.access_token,
            token_response.token_type,
            token_response.expires_in,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{serve_once, served};

    const SECRET: &str = "client-secret-value";

    fn config(base_url: &str) -> Config {
        let vars = [
            ("BASE_RESOURCE_URL", base_url),
            ("CLIENT_ID", "client-id"),
            ("CLIENT_SECRET", SECRET),
            ("ACTION_IDS_RESOURCE_PATHS", "aaa"),
            ("ACTION_ID_CUSTOM_FIELD", "cfactionid"),
        ];
        Config::from_lookup(|key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        })
        .unwrap()
    }

    #[test]
    fn debug_output_redacts_the_client_secret() {
        let config = config("https://halo.example.com/");
        let config_debug = format!("{config:?}");
        let client_debug = format!("{:?}", AuthClient::new(config));
        for debug in [config_debug, client_debug] {
            assert!(!debug.contains(SECRET), "{debug}");
            assert!(debug.contains("client-id"), "{debug}");
        }
    }

    #[tokio::test]
    async fn a_token_response_of_an_unexpected_shape_keeps_its_body_out_of_the_error() {
        let (base_url, server) = serve_once(
            200,
            r#"{"access_token":"tok-abcdef123456","shape":"unexpected"}"#,
        )
        .await;
        let client = AuthClient::new(config(&base_url));

        let result = client.get_valid_token().await;
        let request = served(server).await;
        let message = format!("{:#}", result.unwrap_err());
        assert!(
            request.line.starts_with("POST /auth/token"),
            "{}",
            request.line
        );
        assert!(!message.contains("tok-abcdef123456"), "{message}");
        assert!(message.contains("token response"), "{message}");
    }

    #[tokio::test]
    async fn an_expiry_too_far_out_to_represent_is_an_error_not_a_panic() {
        let (base_url, server) = serve_once(
            200,
            r#"{"access_token":"tok","token_type":"Bearer","expires_in":9223372036854775807}"#,
        )
        .await;
        let client = AuthClient::new(config(&base_url));

        let result = client.get_valid_token().await;
        served(server).await;
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("expires_in"), "{message}");
    }
}
