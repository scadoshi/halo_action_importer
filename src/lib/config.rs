//! Settings read from `.env` and the environment.

use anyhow::Context;
use reqwest::Url;
use tracing::Level;

/// The importer's settings. `Debug` leaves out the client secret.
#[derive(Clone)]
pub struct Config {
    /// The Halo instance's base URL, with its trailing slash.
    pub base_resource_url: Url,
    /// The OAuth 2 token endpoint, `auth/token` under the base URL.
    pub token_url: Url,
    /// The API application's client ID.
    pub client_id: String,
    /// The API application's client secret.
    pub client_secret: String,
    /// The UUIDs of the reports `--sync-from-halo` fetches; empty when unset.
    pub action_ids_resource_uuids: Vec<String>,
    /// The name of the action custom field that holds the action ID, such as `cfactionid`.
    pub action_id_custom_field: String,
    /// The most detailed level that gets logged.
    pub log_level: Level,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_resource_url", &self.base_resource_url)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("action_ids_resource_uuids", &self.action_ids_resource_uuids)
            .field("action_id_custom_field", &self.action_id_custom_field)
            .field("log_level", &self.log_level)
            .finish()
    }
}

const BASE_RESOURCE_URL_KEY: &str = "BASE_RESOURCE_URL";
const CLIENT_ID_KEY: &str = "CLIENT_ID";
const CLIENT_SECRET_KEY: &str = "CLIENT_SECRET";

const TOKEN_URL_PATH: &str = "auth/token";

const ACTION_IDS_RESOURCE_PATH_KEY: &str = "ACTION_IDS_RESOURCE_PATHS";
const ACTION_ID_CUSTOM_FIELD_KEY: &str = "ACTION_ID_CUSTOM_FIELD";
const LOG_LEVEL_KEY: &str = "LOG_LEVEL";

impl Config {
    /// The report UUIDs `--sync-from-halo` fetches.
    ///
    /// # Errors
    ///
    /// When none are configured.
    pub fn report_uuids(&self) -> anyhow::Result<&[String]> {
        if self.action_ids_resource_uuids.is_empty() {
            anyhow::bail!(
                "{ACTION_IDS_RESOURCE_PATH_KEY} must list at least one report UUID to run with --sync-from-halo"
            );
        }
        Ok(&self.action_ids_resource_uuids)
    }

    /// Reads `.env` when present, then the process environment.
    ///
    /// # Errors
    ///
    /// When a required variable is missing or a value is malformed.
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Builds the config from `lookup`, which answers `None` for an unset key.
    ///
    /// # Errors
    ///
    /// When a required variable is missing or a value is malformed; the
    /// message names the variable.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let var = |key: &'static str| {
            lookup(key).with_context(|| format!("missing required environment variable: {key}"))
        };

        let base_url_str = var(BASE_RESOURCE_URL_KEY)?;
        let base_resource_url = Url::try_from(base_url_str.as_str()).with_context(|| {
            format!(
                "invalid URL format for {}: {}",
                BASE_RESOURCE_URL_KEY, base_url_str
            )
        })?;
        let mut token_url = base_resource_url.clone();
        token_url.set_path(TOKEN_URL_PATH);
        let client_id = var(CLIENT_ID_KEY)?;
        let client_secret = var(CLIENT_SECRET_KEY)?;
        let action_ids_uuids_str = lookup(ACTION_IDS_RESOURCE_PATH_KEY).unwrap_or_default();
        let action_ids_resource_uuids: Vec<String> = action_ids_uuids_str
            .split(',')
            .map(|uuid| uuid.trim().to_string())
            .filter(|uuid| !uuid.is_empty())
            .collect();
        let action_id_custom_field = var(ACTION_ID_CUSTOM_FIELD_KEY)?.trim().to_string();
        if action_id_custom_field.is_empty() {
            anyhow::bail!(
                "{ACTION_ID_CUSTOM_FIELD_KEY} is blank: it must name the action ID custom field"
            );
        }

        let log_level_str = lookup(LOG_LEVEL_KEY).unwrap_or_else(|| "info".to_string());
        let log_level_str_trimmed = log_level_str.trim().to_lowercase();
        let log_level = match log_level_str_trimmed.as_str() {
            "trace" => Level::TRACE,
            "debug" => Level::DEBUG,
            "info" => Level::INFO,
            "warn" => Level::WARN,
            "error" => Level::ERROR,
            _ => {
                anyhow::bail!(
                    "invalid log level '{}' for {}. must be one of: trace, debug, info, warn, error",
                    log_level_str_trimmed,
                    LOG_LEVEL_KEY
                );
            }
        };

        Ok(Self {
            base_resource_url,
            token_url,
            client_id,
            client_secret,
            action_ids_resource_uuids,
            action_id_custom_field,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    const COMPLETE: &[(&str, &str)] = &[
        ("BASE_RESOURCE_URL", "https://halo.example.com/api/"),
        ("CLIENT_ID", "id"),
        ("CLIENT_SECRET", "secret"),
        ("ACTION_IDS_RESOURCE_PATHS", " aaa, bbb,,ccc "),
        ("ACTION_ID_CUSTOM_FIELD", " cfactionid "),
    ];

    #[test]
    fn a_complete_set_parses_and_derives_the_token_url() {
        let config = Config::from_lookup(vars(COMPLETE)).unwrap();
        assert_eq!(
            config.token_url.as_str(),
            "https://halo.example.com/auth/token"
        );
        assert_eq!(config.action_ids_resource_uuids, ["aaa", "bbb", "ccc"]);
        assert_eq!(config.action_id_custom_field, "cfactionid");
        assert_eq!(config.log_level, Level::INFO);
    }

    #[test]
    fn the_report_ids_are_optional_until_a_sync_asks_for_them() {
        let without: Vec<(&str, &str)> = COMPLETE
            .iter()
            .copied()
            .filter(|(k, _)| *k != "ACTION_IDS_RESOURCE_PATHS")
            .collect();
        let config = Config::from_lookup(vars(&without)).unwrap();
        let message = config.report_uuids().unwrap_err().to_string();
        assert!(message.contains("ACTION_IDS_RESOURCE_PATHS"), "{message}");
        assert!(message.contains("--sync-from-halo"), "{message}");

        let config = Config::from_lookup(vars(COMPLETE)).unwrap();
        assert_eq!(config.report_uuids().unwrap(), ["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn a_missing_variable_is_named() {
        for (key, _) in COMPLETE
            .iter()
            .filter(|(k, _)| *k != "ACTION_IDS_RESOURCE_PATHS")
        {
            let without: Vec<(&str, &str)> =
                COMPLETE.iter().copied().filter(|(k, _)| k != key).collect();
            let message = Config::from_lookup(vars(&without)).unwrap_err().to_string();
            assert!(message.contains(key), "{message}");
        }
    }

    #[test]
    fn a_bad_url_a_blank_field_name_and_a_bad_log_level_are_named() {
        let with = |key: &str, value: &str| {
            let mut pairs: Vec<(&str, &str)> = COMPLETE
                .iter()
                .copied()
                .filter(|(k, _)| *k != key)
                .collect();
            pairs.push((key, value));
            Config::from_lookup(vars(&pairs)).unwrap_err().to_string()
        };
        assert!(with("BASE_RESOURCE_URL", "not a url").contains("BASE_RESOURCE_URL"));
        assert!(with("ACTION_ID_CUSTOM_FIELD", "  ").contains("ACTION_ID_CUSTOM_FIELD"));
        assert!(with("LOG_LEVEL", "loud").contains("LOG_LEVEL"));
    }

    #[test]
    fn the_log_level_is_case_insensitive_and_trimmed() {
        let mut pairs = COMPLETE.to_vec();
        pairs.push(("LOG_LEVEL", " Debug "));
        assert_eq!(
            Config::from_lookup(vars(&pairs)).unwrap().log_level,
            Level::DEBUG
        );
    }
}
