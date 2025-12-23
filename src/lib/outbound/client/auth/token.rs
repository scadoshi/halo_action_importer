use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthToken {
    access_token: String,
    token_type: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    expires_at: DateTime<Utc>,
}

impl AuthToken {
    pub fn new(access_token: String, token_type: String, expires_in: i64) -> Self {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        Self {
            access_token,
            token_type,
            expires_at,
        }
    }

    pub fn is_expired(&self) -> bool {
        // Add a small buffer (30 seconds) to prevent edge cases
        Utc::now() + Duration::seconds(30) >= self.expires_at
    }

    pub fn header_value(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
    }
}
