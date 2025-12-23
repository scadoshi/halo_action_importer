use url::Url;

#[derive(Debug, Clone)]
pub struct Config {
    pub base_resource_url: Url,
    pub token_url: Url,
    pub client_id: String,
    pub client_secret: String,
    pub action_ids_resource: Url,
    pub action_id_custom_field_id: u32,
}

const BASE_RESOURCE_URL_KEY: &str = "BASE_RESOURCE_URL";
const CLIENT_ID_KEY: &str = "CLIENT_ID";
const CLIENT_SECRET_KEY: &str = "CLIENT_SECRET";

const TOKEN_URL_PATH: &str = "auth/token";

const ACTION_IDS_RESOURCE_PATH_KEY: &str = "ACTION_IDS_RESOURCE_PATH";
const ACTION_ID_CUSTOM_FIELD_ID_KEY: &str = "ACTION_ID_CUSTOM_FIELD_ID";

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        let base_resource_url = Url::try_from(std::env::var(BASE_RESOURCE_URL_KEY)?.as_str())?;
        let mut token_url = base_resource_url.clone();
        token_url.set_path(TOKEN_URL_PATH);
        let client_id = std::env::var(CLIENT_ID_KEY)?;
        let client_secret = std::env::var(CLIENT_SECRET_KEY)?;
        let mut action_ids_resource = base_resource_url.clone();
        action_ids_resource.set_path(std::env::var(ACTION_IDS_RESOURCE_PATH_KEY)?.as_str());
        let action_id_custom_field_id = std::env::var(ACTION_ID_CUSTOM_FIELD_ID_KEY)?.parse()?;

        Ok(Self {
            base_resource_url,
            token_url,
            client_id,
            client_secret,
            action_ids_resource,
            action_id_custom_field_id,
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
