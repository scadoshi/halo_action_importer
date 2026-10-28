use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::domain::importer::format::format_number;
use crate::domain::importer::setup::{Cache, parse_action_ids};
use crate::domain::models::action_object::ActionId;
use crate::outbound::client::auth::AuthClient;

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
        cache: &Cache,
        already_fetched: &HashSet<String>,
    ) -> anyhow::Result<HashSet<ActionId>> {
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
                .fetch_token()
                .await
                .context("Failed to get valid authentication token")?;
            // A 401 gets one new token and one more try; a second 401 is an error.
            let mut refreshed = false;

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
                        auth_token = self.auth_client.fetch_token().await.context(
                            "Failed to refresh authentication token after network error",
                        )?;
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
                        .fetch_token()
                        .await
                        .context("Failed to refresh authentication token after 504")?;
                    continue;
                }

                if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
                    warn!(
                        "Received 401 Unauthorized for report request, refreshing token and retrying"
                    );
                    auth_token = self
                        .auth_client
                        .refresh_token()
                        .await
                        .context("Failed to refresh authentication token after 401")?;
                    refreshed = true;
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

                let report_ids = parse_action_ids(
                    report_data.iter().flat_map(|row| row.action_ids.split(',')),
                    &format!("report {uuid}"),
                );
                all_existing_ids.extend(report_ids.iter().copied());

                if let Err(e) = cache.merge_report(uuid, &report_ids) {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::test_server::{Reply, TOKEN, TestServer, config, lines, scratch_dir};

    /// How long a test lets a fetch run before calling it hung.
    const HUNG: Duration = Duration::from_secs(5);

    const POST_TOKEN: &str = "POST /auth/token HTTP/1.1";
    const GET_REPORT: &str = "GET /api/ReportData/aaa HTTP/1.1";
    const REPORT: Reply = Reply::Json(200, r#"[{"group_num":"1","action_ids":"1,2,3"}]"#);

    fn client(server: &TestServer) -> ReportClient {
        let config = config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()));
        ReportClient::new(config, auth_client)
    }

    /// Fetches every report into a scratch cache named `name`.
    async fn fetch(server: &TestServer, name: &str) -> anyhow::Result<HashSet<ActionId>> {
        let dir = scratch_dir(name);
        let cache = Cache::new(&dir);
        let result = tokio::time::timeout(
            HUNG,
            client(server).get_existing_action_ids(&cache, &HashSet::new()),
        )
        .await
        .expect("the report fetch hung");
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    fn ids(ids: &[u64]) -> HashSet<ActionId> {
        ids.iter().copied().map(ActionId::new).collect()
    }

    #[tokio::test]
    async fn a_report_answered_401_fetches_a_new_token_and_asks_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(401, "{}"), TOKEN, REPORT]).await;

        let result = fetch(&server, "report_401_once").await;

        assert_eq!(result.unwrap(), ids(&[1, 2, 3]));
        assert_eq!(
            lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, POST_TOKEN, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_answered_401_twice_is_an_error() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(401, "{}"),
            TOKEN,
            Reply::Json(401, "{}"),
        ])
        .await;

        let result = fetch(&server, "report_401_twice").await;

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("401"), "{message}");
        assert_eq!(
            lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, POST_TOKEN, GET_REPORT]
        );
    }
}
