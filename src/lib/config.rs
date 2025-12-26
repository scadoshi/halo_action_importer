use anyhow::Context;
use reqwest::Url;
use tracing::Level;

#[derive(Debug, Clone)]
pub struct Config {
    pub base_resource_url: Url,
    pub token_url: Url,
    pub client_id: String,
    pub client_secret: String,
    pub action_ids_resources: Vec<Url>,
    pub action_id_custom_field_id: u32,
    pub log_level: Level,
}

const BASE_RESOURCE_URL_KEY: &str = "BASE_RESOURCE_URL";
const CLIENT_ID_KEY: &str = "CLIENT_ID";
const CLIENT_SECRET_KEY: &str = "CLIENT_SECRET";

const TOKEN_URL_PATH: &str = "auth/token";

const ACTION_IDS_RESOURCE_PATH_KEY: &str = "ACTION_IDS_RESOURCE_PATH";
const ACTION_ID_CUSTOM_FIELD_ID_KEY: &str = "ACTION_ID_CUSTOM_FIELD_ID";
const LOG_LEVEL_KEY: &str = "LOG_LEVEL";

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        let base_url_str = std::env::var(BASE_RESOURCE_URL_KEY).with_context(|| {
            format!(
                "missing required environment variable: {}",
                BASE_RESOURCE_URL_KEY
            )
        })?;
        let base_resource_url = Url::try_from(base_url_str.as_str()).with_context(|| {
            format!(
                "invalid URL format for {}: {}",
                BASE_RESOURCE_URL_KEY, base_url_str
            )
        })?;
        let mut token_url = base_resource_url.clone();
        token_url.set_path(TOKEN_URL_PATH);
        let client_id = std::env::var(CLIENT_ID_KEY)
            .with_context(|| format!("missing required environment variable: {}", CLIENT_ID_KEY))?;
        let client_secret = std::env::var(CLIENT_SECRET_KEY).with_context(|| {
            format!(
                "missing required environment variable: {}",
                CLIENT_SECRET_KEY
            )
        })?;
        let action_ids_paths = std::env::var(ACTION_IDS_RESOURCE_PATH_KEY).with_context(|| {
            format!(
                "missing required environment variable: {}",
                ACTION_IDS_RESOURCE_PATH_KEY
            )
        })?;
        let action_ids_resources: Vec<Url> = action_ids_paths
            .split(',')
            .map(|path| {
                let path = path.trim();
                let mut url = base_resource_url.clone();
                url.set_path(path);
                url
            })
            .collect();
        let action_id_custom_field_id_str = std::env::var(ACTION_ID_CUSTOM_FIELD_ID_KEY)
            .with_context(|| {
                format!(
                    "missing required environment variable: {}",
                    ACTION_ID_CUSTOM_FIELD_ID_KEY
                )
            })?;
        let action_id_custom_field_id =
            action_id_custom_field_id_str.parse().with_context(|| {
                format!(
                    "invalid integer format for {}: {}",
                    ACTION_ID_CUSTOM_FIELD_ID_KEY, action_id_custom_field_id_str
                )
            })?;

        let log_level_str = std::env::var(LOG_LEVEL_KEY).unwrap_or_else(|_| "info".to_string());
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
            action_ids_resources,
            action_id_custom_field_id,
            log_level,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_from_env() {
        assert!(Config::from_env().is_ok());
    }
}
