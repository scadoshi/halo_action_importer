pub mod token;

use crate::{config::Config, outbound::client::auth::token::AuthToken};
use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::error;

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Serialize)]
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
        *token_guard = Some(new_token);

        Ok(token_guard.as_ref().unwrap().header_value())
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

        let token_response: TokenResponse =
            match serde_json::from_str(&response_text).with_context(|| {
                format!(
                    "failed to parse token response JSON (status: {}): {}",
                    status, response_text
                )
            }) {
                Ok(token) => token,
                Err(e) => {
                    error!("Failed to parse token response: {}", e);
                    return Err(e);
                }
            };

        Ok(AuthToken::new(
            token_response.access_token,
            token_response.token_type,
            token_response.expires_in,
        ))
    }
}
