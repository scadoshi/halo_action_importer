use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::{Client as ReqwestClient, Response, StatusCode};
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::domain::importer::setup::Cache;
use crate::domain::importer::{format, setup};
use crate::domain::models::action_object::ActionId;
use crate::outbound::client;
use crate::outbound::client::auth::AuthClient;
use crate::outbound::client::{MAX_ATTEMPTS, RETRY_BACKOFF};

#[derive(Debug, Deserialize)]
struct ReportResponse {
    #[serde(rename = "group_num")]
    _group_num: String,
    #[serde(rename = "action_ids")]
    action_ids: String,
}

/// Fetches the action ids Halo's reports hold. A fetch that fails with a
/// network error, a timeout or a 504 is tried again after a pause, up to
/// [`MAX_ATTEMPTS`] times in all; a 401 gets a new token and one more try.
#[derive(Debug, Clone)]
pub struct ReportClient {
    config: Config,
    http_client: ReqwestClient,
    auth_client: Arc<AuthClient>,
    retry_backoff: Duration,
}

impl ReportClient {
    /// # Errors
    ///
    /// When the HTTP client cannot be built.
    pub fn new(config: Config, auth_client: Arc<AuthClient>) -> anyhow::Result<Self> {
        Self::with_request_timeout(config, auth_client, client::REQUEST_TIMEOUT)
    }

    /// A report client whose requests give up after `timeout`.
    pub(crate) fn with_request_timeout(
        config: Config,
        auth_client: Arc<AuthClient>,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config,
            http_client: client::http_client(timeout)?,
            auth_client,
            retry_backoff: RETRY_BACKOFF,
        })
    }

    /// Waits `backoff` between attempts instead of [`RETRY_BACKOFF`].
    #[cfg(test)]
    fn with_retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }

    /// Fetches every configured report, merges each one's ids into `cache`
    /// as it arrives, and returns all the ids.
    ///
    /// # Errors
    ///
    /// When a report cannot be fetched or read, or is empty.
    pub async fn fetch_existing_action_ids(
        &self,
        cache: &Cache,
    ) -> anyhow::Result<HashSet<ActionId>> {
        let mut all_existing_ids = HashSet::new();
        let to_fetch = self.config.report_uuids()?;
        info!("Fetching {} report resource(s)", to_fetch.len());

        for (idx, uuid) in to_fetch.iter().enumerate() {
            let report_url = format!("{}api/ReportData/{}", self.config.base_resource_url, uuid);
            info!("Fetching report {}/{}: {}", idx + 1, to_fetch.len(), uuid);

            let report_data = self.fetch_report(&report_url, uuid).await?;

            if report_data.is_empty() {
                error!("Report response is empty");
                anyhow::bail!("Report response is empty");
            }

            let report_ids = setup::parse_action_ids(
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
                format::format_number(report_ids.len()),
                uuid,
                format::format_number(all_existing_ids.len())
            );
        }

        info!(
            "Completed fetching reports: {} new action IDs from {} resource(s)",
            format::format_number(all_existing_ids.len()),
            to_fetch.len()
        );
        Ok(all_existing_ids)
    }

    /// Fetches one report's rows, trying again on a network error, a timeout
    /// or a 504 until [`MAX_ATTEMPTS`] attempts have failed.
    async fn fetch_report(
        &self,
        report_url: &str,
        uuid: &str,
    ) -> anyhow::Result<Vec<ReportResponse>> {
        let mut auth_token = self
            .auth_client
            .fetch_token()
            .await
            .context("Failed to get valid authentication token")?;
        // A 401 gets one new token and one more try; a second 401 is an error.
        let mut refreshed = false;
        let mut attempt = 1;

        loop {
            let failure = match self
                .http_client
                .get(report_url)
                .header("Authorization", &auth_token)
                .header("Content-Type", "application/json; charset=utf-8")
                .send()
                .await
            {
                Err(e) => format!("network error fetching report {uuid}: {e}"),
                Ok(response) if response.status() == StatusCode::GATEWAY_TIMEOUT => {
                    format!("report {uuid} answered 504 Gateway Timeout")
                }
                Ok(response) if response.status() == StatusCode::UNAUTHORIZED && !refreshed => {
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
                Ok(response) => return read_report(response).await,
            };

            if attempt >= MAX_ATTEMPTS {
                error!("Giving up on report {uuid} after {attempt} attempts: {failure}");
                anyhow::bail!("Giving up on report {uuid} after {attempt} attempts: {failure}");
            }
            warn!(
                "Attempt {attempt} of {MAX_ATTEMPTS} failed: {failure}; trying again in {}s",
                self.retry_backoff.as_secs()
            );
            tokio::time::sleep(self.retry_backoff).await;
            attempt += 1;
            auth_token = self
                .auth_client
                .fetch_token()
                .await
                .context("Failed to get valid authentication token")?;
        }
    }
}

/// Reads a report's rows from a response that is not a 504 or a first 401.
async fn read_report(response: Response) -> anyhow::Result<Vec<ReportResponse>> {
    let status = response.status();
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

    match response
        .json()
        .await
        .context("failed to parse report response")
    {
        Ok(data) => Ok(data),
        Err(e) => {
            error!("Failed to parse report response: {}", e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::test_server;
    use crate::test_server::{Reply, SHORT_TIMEOUT, TOKEN, TestServer};

    /// How long a test lets a fetch run before calling it hung.
    const HUNG: Duration = Duration::from_secs(5);

    const POST_TOKEN: &str = "POST /auth/token HTTP/1.1";
    const GET_REPORT: &str = "GET /api/ReportData/aaa HTTP/1.1";
    const REPORT: Reply = Reply::Json(200, r#"[{"group_num":"1","action_ids":"1,2,3"}]"#);

    fn client(server: &TestServer) -> ReportClient {
        let config = test_server::config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        ReportClient::new(config, auth_client)
            .unwrap()
            .with_retry_backoff(Duration::ZERO)
    }

    /// Fetches every report into a scratch cache named `name`.
    async fn fetch(server: &TestServer, name: &str) -> anyhow::Result<HashSet<ActionId>> {
        let dir = test_server::scratch_dir(name);
        let cache = Cache::new(&dir);
        let result = tokio::time::timeout(HUNG, client(server).fetch_existing_action_ids(&cache))
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
            test_server::lines(&server.requests()),
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
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, POST_TOKEN, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_answered_504_on_every_attempt_is_an_error_after_three() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(504, "{}"),
            Reply::Json(504, "{}"),
            Reply::Json(504, "{}"),
        ])
        .await;

        let result = fetch(&server, "report_504_always").await;

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("504"), "{message}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, GET_REPORT, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_answered_504_is_asked_again_after_the_backoff() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(504, "{}"), REPORT]).await;
        let backoff = Duration::from_millis(300);
        let dir = test_server::scratch_dir("report_504_backoff");
        let config = test_server::config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        let client = ReportClient::new(config, auth_client)
            .unwrap()
            .with_retry_backoff(backoff);

        let started = std::time::Instant::now();
        let result = client.fetch_existing_action_ids(&Cache::new(&dir)).await;
        let elapsed = started.elapsed();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), ids(&[1, 2, 3]));
        assert!(elapsed >= backoff, "{elapsed:?}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_hit_by_a_network_error_is_asked_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Hangup, REPORT]).await;

        let result = fetch(&server, "report_hangup").await;

        assert_eq!(result.unwrap(), ids(&[1, 2, 3]));
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_that_times_out_is_asked_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Stall, REPORT]).await;
        let dir = test_server::scratch_dir("report_timeout");
        let config = test_server::config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        let client = ReportClient::with_request_timeout(config, auth_client, SHORT_TIMEOUT)
            .unwrap()
            .with_retry_backoff(Duration::ZERO);

        let result =
            tokio::time::timeout(HUNG, client.fetch_existing_action_ids(&Cache::new(&dir)))
                .await
                .expect("the report fetch hung");
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.unwrap(), ids(&[1, 2, 3]));
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_that_is_not_json_is_an_error_and_not_asked_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(200, "<html>oops</html>")]).await;

        let result = fetch(&server, "report_not_json").await;

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("parse report response"), "{message}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn a_report_answered_with_another_error_status_is_an_error_and_not_asked_again() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(500, r#"{"error":"boom"}"#)]).await;

        let result = fetch(&server, "report_500").await;

        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("500"), "{message}");
        assert_eq!(
            test_server::lines(&server.requests()),
            [POST_TOKEN, GET_REPORT]
        );
    }

    #[tokio::test]
    async fn fetched_report_ids_are_merged_into_the_cache() {
        let server = TestServer::start(vec![TOKEN, REPORT]).await;
        let dir = test_server::scratch_dir("report_cached");
        let cache = Cache::new(&dir);

        client(&server)
            .fetch_existing_action_ids(&cache)
            .await
            .unwrap();

        let cached = cache.read().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(cached.action_ids, ids(&[1, 2, 3]));
        assert!(cached.fetched_resources.contains("aaa"));
    }
}
