use crate::{
    config::Config, domain::models::action_object::ActionObject, outbound::client::auth::AuthClient,
};
use anyhow::Context;
use reqwest::Client as ReqwestClient;
use std::sync::Arc;
use tracing::{error, warn};

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
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let action_ids: Vec<String> = action_objects
            .iter()
            .map(|a| a.action_id().to_string())
            .collect();
        let mut endpoint = self.config.base_resource_url.clone();
        endpoint.set_path("api/actions");
        let endpoint_str = endpoint.to_string();
        let mut auth_token = self
            .auth_client
            .get_valid_token()
            .await
            .context("Failed to get valid authentication token")?;
        for attempt in 0..2 {
            let request = self
                .http_client
                .post(endpoint.clone())
                .header("Authorization", &auth_token)
                .header("Content-Type", "application/json; charset=utf-8")
                .json(&action_objects);

            let response = match request.send().await.with_context(|| {
                format!(
                    "failed to send POST request for action IDs: {:?} to endpoint: {}",
                    action_ids, endpoint_str
                )
            }) {
                Ok(resp) => resp,
                Err(e) => {
                    error!(
                        "Failed to send POST request for action IDs {:?}: {}",
                        action_ids, e
                    );
                    return Err(e);
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                warn!("Received 401 Unauthorized for batch, refreshing token and retrying");
                auth_token = self
                    .auth_client
                    .get_valid_token()
                    .await
                    .context("Failed to refresh authentication token after 401")?;
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
        anyhow::bail!("Failed to post action objects after retry")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::models::action_object::ActionId, outbound::client::auth::AuthClient};
    use std::sync::Arc;
    #[tokio::test]
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
            ActionId::new("897"),
        );
        let response = action_client.post_action_objects(vec![action_object]).await;
        assert!(response.is_ok());
    }
}
