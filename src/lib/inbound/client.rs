use crate::{
    config::Config, domain::importer::setup::append_resource_to_cache,
    outbound::client::auth::AuthClient,
};
use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{error, info, warn};

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
    #[serde(rename = "group_num")]
    _group_num: String,
    #[serde(rename = "action_ids")]
    action_ids: String,
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

    pub async fn get_existing_action_ids(
        &self,
        already_fetched: &HashSet<String>,
    ) -> anyhow::Result<HashSet<String>> {
        let mut all_existing_ids = HashSet::new();
        let total_reports = self.config.action_ids_resource_uuids.len();
        let to_fetch: Vec<_> = self
            .config
            .action_ids_resource_uuids
            .iter()
            .filter(|uuid| !already_fetched.contains(*uuid))
            .collect();
        let skipped = total_reports - to_fetch.len();

        info!(
            "Found {} report resource(s), {} already cached, {} to fetch",
            total_reports,
            skipped,
            to_fetch.len()
        );

        if to_fetch.is_empty() {
            info!("All resources already cached, skipping report fetching");
            return Ok(all_existing_ids);
        }

        for (idx, uuid) in to_fetch.iter().enumerate() {
            let report_url = format!("{}api/ReportData/{}", self.config.base_resource_url, uuid);
            info!("Fetching report {}/{}: {}", idx + 1, to_fetch.len(), uuid);

            let mut auth_token = self
                .auth_client
                .get_valid_token()
                .await
                .context("Failed to get valid authentication token")?;

            loop {
                let response = match self
                    .http_client
                    .get(&report_url)
                    .header("Authorization", &auth_token)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .send()
                    .await
                {
                    Ok(resp) => resp,
                    Err(e) => {
                        // Network error, timeout, or connection failure - retry indefinitely
                        warn!(
                            "Network error fetching report {} ({}): {} - retrying immediately",
                            idx + 1,
                            uuid,
                            e
                        );
                        auth_token = self
                            .auth_client
                            .get_valid_token()
                            .await
                            .context("Failed to refresh authentication token after network error")?;
                        continue;
                    }
                };

                let status = response.status();

                if status == reqwest::StatusCode::GATEWAY_TIMEOUT {
                    warn!(
                        "Received 504 Gateway Timeout for report {} ({}), retrying immediately",
                        idx + 1,
                        uuid
                    );
                    auth_token = self
                        .auth_client
                        .get_valid_token()
                        .await
                        .context("Failed to refresh authentication token after 504")?;
                    continue;
                }

                if status == reqwest::StatusCode::UNAUTHORIZED {
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

                if report_data.is_empty() {
                    error!("Report response is empty");
                    anyhow::bail!("Report response is empty");
                }

                let mut report_ids: Vec<String> = Vec::new();
                for row in &report_data {
                    for id_str in row.action_ids.split(',') {
                        let id_str = id_str.trim();
                        if !id_str.is_empty() {
                            report_ids.push(id_str.to_string());
                            all_existing_ids.insert(id_str.to_string());
                        }
                    }
                }

                if let Err(e) = append_resource_to_cache(uuid, &report_ids) {
                    warn!("Failed to cache IDs from report {}: {}", uuid, e);
                }

                info!(
                    "Report {}/{} complete: {} IDs from {}, {} total new IDs",
                    idx + 1,
                    to_fetch.len(),
                    format_number(report_ids.len()),
                    uuid,
                    format_number(all_existing_ids.len())
                );
                break;
            }
        }

        info!(
            "Completed fetching reports: {} new action IDs from {} resource(s)",
            format_number(all_existing_ids.len()),
            to_fetch.len()
        );
        Ok(all_existing_ids)
    }
}
