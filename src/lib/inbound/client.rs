use crate::{config::Config, outbound::client::auth::AuthClient};
use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{error, warn};

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

#[derive(Debug, Deserialize)]
struct ReportResponse {
    #[serde(rename = "existingActionIds")]
    existing_action_ids: String,
}

#[derive(Debug, Clone)]
pub struct ReportClient {
    config: Config,
    http_client: ReqwestClient,
    auth_client: Arc<AuthClient>,
}

impl ReportClient {
    pub fn new(config: Config, auth_client: Arc<AuthClient>) -> Self {
        Self {
            config,
            http_client: ReqwestClient::new(),
            auth_client,
        }
    }

    pub async fn get_existing_action_ids(&self) -> anyhow::Result<HashSet<String>> {
        let mut auth_token = self
            .auth_client
            .get_valid_token()
            .await
            .context("Failed to get valid authentication token")?;
        for attempt in 0..2 {
            let response = self
                .http_client
                .get(self.config.action_ids_resource.as_str())
                .header("Authorization", &auth_token)
                .header("Content-Type", "application/json; charset=utf-8")
                .send()
                .await
                .context("failed to send report request")?;

            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                warn!(
                    "Received 401 Unauthorized for report request, refreshing token and retrying"
                );
                auth_token = self
                    .auth_client
                    .get_valid_token()
                    .await
                    .context("Failed to refresh authentication token after 401")?;
                continue;
            }

            if !status.is_success() {
                let error_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "failed to get error response".to_string());
                error!(
                    "Report request failed: status {}, error: {}",
                    status, error_text
                );
                anyhow::bail!(
                    "Report request failed: status {}, error: {}",
                    status,
                    error_text
                );
            }
            let report_data: Vec<ReportResponse> = match response
                .json()
                .await
                .context("failed to parse report response")
            {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to parse report response: {}", e);
                    return Err(e);
                }
            };

            let existing_ids_str = match report_data.first() {
                Some(data) => data.existing_action_ids.clone(),
                None => {
                    error!("Report response is empty");
                    anyhow::bail!("Report response is empty");
                }
            };

            let mut existing_ids = HashSet::new();
            for id_str in existing_ids_str.split(',') {
                let id_str = id_str.trim();
                if !id_str.is_empty() {
                    existing_ids.insert(id_str.to_string());
                }
            }

            tracing::info!(
                "Found {} existing action IDs",
                format_number(existing_ids.len())
            );
            return Ok(existing_ids);
        }
        anyhow::bail!("Failed to get existing action IDs after retry")
    }
}
