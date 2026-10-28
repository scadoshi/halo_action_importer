pub mod action;
pub mod auth;

use std::time::Duration;

use anyhow::Context;
use reqwest::Client as ReqwestClient;

/// How long one request may take, start to finish. A report serving millions
/// of ids can take minutes; a request still going after this is given up.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// How many times a report fetch is tried before its error stands.
pub const MAX_ATTEMPTS: u32 = 3;

/// How long to wait before trying a report fetch again.
pub const RETRY_BACKOFF: Duration = Duration::from_secs(60);

/// An HTTP client whose requests give up after `timeout`.
pub(crate) fn http_client(timeout: Duration) -> anyhow::Result<ReqwestClient> {
    ReqwestClient::builder()
        .timeout(timeout)
        .build()
        .context("failed to build the HTTP client")
}
