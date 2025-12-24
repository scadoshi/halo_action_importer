use crate::config::Config;
use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use std::collections::HashSet;
use tracing::error;

#[derive(Debug, Deserialize)]
struct ReportResponse {
    #[serde(rename = "existingActionIds")]
    existing_action_ids: String,
}

#[derive(Debug, Clone)]
pub struct ReportClient {
    config: Config,
    http_client: ReqwestClient,
    auth_token: String,
}

impl ReportClient {
    pub fn new(config: Config, auth_token: String) -> Self {
        Self {
            config,
            http_client: ReqwestClient::new(),
            auth_token,
        }
    }

    pub async fn get_existing_action_ids(&self) -> anyhow::Result<HashSet<String>> {
        let response = self
            .http_client
            .get(self.config.action_ids_resource.as_str())
            .header("Authorization", &self.auth_token)
            .header("Content-Type", "application/json; charset=utf-8")
            .send()
            .await
            .context("failed to send report request")?;

        if !response.status().is_success() {
            let status = response.status();
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

        tracing::info!("Found {} existing action IDs", existing_ids.len());
        Ok(existing_ids)
    }
}
