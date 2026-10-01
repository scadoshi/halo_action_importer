use anyhow::Context;
use reqwest::Url;
use tracing::Level;

#[derive(Debug, Clone)]
pub struct Config {
    pub base_resource_url: Url,
    pub token_url: Url,
    pub client_id: String,
    pub client_secret: String,
    pub action_ids_resource_uuids: Vec<String>,
    pub action_id_custom_field_id: u32,
    pub log_level: Level,
}

const BASE_RESOURCE_URL_KEY: &str = "BASE_RESOURCE_URL";
const CLIENT_ID_KEY: &str = "CLIENT_ID";
const CLIENT_SECRET_KEY: &str = "CLIENT_SECRET";

const TOKEN_URL_PATH: &str = "auth/token";

const ACTION_IDS_RESOURCE_PATH_KEY: &str = "ACTION_IDS_RESOURCE_PATHS";
const ACTION_ID_CUSTOM_FIELD_ID_KEY: &str = "ACTION_ID_CUSTOM_FIELD_ID";
const LOG_LEVEL_KEY: &str = "LOG_LEVEL";

impl Config {
    /// Reads `.env` when present, then the process environment.
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Builds the config from `lookup`, which answers `None` for an unset key.
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
        let action_ids_uuids_str = var(ACTION_IDS_RESOURCE_PATH_KEY)?;
        let action_ids_resource_uuids: Vec<String> = action_ids_uuids_str
            .split(',')
            .map(|uuid| uuid.trim().to_string())
            .filter(|uuid| !uuid.is_empty())
            .collect();
        let action_id_custom_field_id_str = var(ACTION_ID_CUSTOM_FIELD_ID_KEY)?;
        let action_id_custom_field_id =
            action_id_custom_field_id_str.parse().with_context(|| {
                format!(
                    "invalid integer format for {}: {}",
                    ACTION_ID_CUSTOM_FIELD_ID_KEY, action_id_custom_field_id_str
                )
            })?;

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
            action_id_custom_field_id,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        ("ACTION_ID_CUSTOM_FIELD_ID", "42"),
    ];

    #[test]
    fn a_complete_set_parses_and_derives_the_token_url() {
        let config = Config::from_lookup(vars(COMPLETE)).unwrap();
        assert_eq!(
            config.token_url.as_str(),
            "https://halo.example.com/auth/token"
        );
        assert_eq!(config.action_ids_resource_uuids, ["aaa", "bbb", "ccc"]);
        assert_eq!(config.action_id_custom_field_id, 42);
        assert_eq!(config.log_level, Level::INFO);
    }

    #[test]
    fn a_missing_variable_is_named() {
        for (key, _) in COMPLETE {
            let without: Vec<(&str, &str)> =
                COMPLETE.iter().copied().filter(|(k, _)| k != key).collect();
            let message = Config::from_lookup(vars(&without)).unwrap_err().to_string();
            assert!(message.contains(key), "{message}");
        }
    }

    #[test]
    fn a_bad_url_a_bad_field_id_and_a_bad_log_level_are_named() {
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
        assert!(
            with("ACTION_ID_CUSTOM_FIELD_ID", "forty-two").contains("ACTION_ID_CUSTOM_FIELD_ID")
        );
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
