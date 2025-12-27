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

    pub async fn get_existing_action_ids(&self) -> anyhow::Result<HashSet<String>> {
        let mut all_existing_ids = HashSet::new();
        let total_reports = self.config.action_ids_resources.len();
        tracing::info!(
            "Fetching existing action IDs from {} report(s)",
            total_reports
        );

        for (idx, report_url) in self.config.action_ids_resources.iter().enumerate() {
            tracing::info!(
                "Fetching report {}/{}: {}",
                idx + 1,
                total_reports,
                report_url
            );

            let mut auth_token = self
                .auth_client
                .get_valid_token()
                .await
                .context("Failed to get valid authentication token")?;

            // Outer loop for 504 timeout retries (infinite until success)
            'outer: loop {
                // Inner loop for 401 auth retries (max 2 attempts)
                for attempt in 0..2 {
                    let response = self
                        .http_client
                        .get(report_url.as_str())
                        .header("Authorization", &auth_token)
                        .header("Content-Type", "application/json; charset=utf-8")
                        .send()
                        .await
                        .context("failed to send report request")?;

                    let status = response.status();
                    
                    // Handle 504 Gateway Timeout - wait 5 minutes and retry from outer loop
                    if status == reqwest::StatusCode::GATEWAY_TIMEOUT {
                        warn!(
                            "Received 504 Gateway Timeout for report {}/{}, waiting 1 minute before retrying",
                            idx + 1,
                            total_reports
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        auth_token = self
                            .auth_client
                            .get_valid_token()
                            .await
                            .context("Failed to refresh authentication token after 504")?;
                        continue 'outer; // Continue outer loop to retry
                    }
                    
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

                    if report_data.is_empty() {
                        error!("Report response is empty");
                        anyhow::bail!("Report response is empty");
                    }

                    for row in &report_data {
                        for id_str in row.action_ids.split(',') {
                            let id_str = id_str.trim();
                            if !id_str.is_empty() {
                                all_existing_ids.insert(id_str.to_string());
                            }
                        }
                    }

                    tracing::info!(
                        "Report {}/{} complete: {} IDs in this report, {} total IDs so far",
                        idx + 1,
                        total_reports,
                        report_data.iter().fold(0, |acc, row| {
                            acc + row
                                .action_ids
                                .split(',')
                                .filter(|s| !s.trim().is_empty())
                                .count()
                        }),
                        format_number(all_existing_ids.len())
                    );
                    break 'outer; // Success - break outer loop and move to next report
                }
            }
        }

        tracing::info!(
            "Completed fetching all reports: {} total existing action IDs",
            format_number(all_existing_ids.len())
        );
        Ok(all_existing_ids)
    }
}
