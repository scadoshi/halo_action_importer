use anyhow::Context;
use chrono::{DateTime, Duration, Utc};

/// How long before its expiry a token counts as expired, so a request never
/// goes out with a token that lapses on the way.
const EXPIRY_MARGIN: Duration = Duration::seconds(30);

pub struct AuthToken {
    access_token: String,
    token_type: String,
    expires_at: DateTime<Utc>,
}

impl AuthToken {
    /// A token that expires `expires_in` seconds from now. An expiry too far
    /// out to represent is an error.
    pub fn new(access_token: String, token_type: String, expires_in: i64) -> anyhow::Result<Self> {
        let expires_at = Duration::try_seconds(expires_in)
            .and_then(|lifetime| Utc::now().checked_add_signed(lifetime))
            .with_context(|| format!("token expires_in of {expires_in} seconds is out of range"))?;
        Ok(Self {
            access_token,
            token_type,
            expires_at,
        })
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() + EXPIRY_MARGIN >= self.expires_at
    }

    pub fn header_value(&self) -> String {
        format!("{} {}", self.token_type, self.access_token)
    }
}

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthToken")
            .field("access_token", &"[redacted]")
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_the_access_token() {
        let token =
            AuthToken::new("tok-abcdef123456".to_string(), "Bearer".to_string(), 3600).unwrap();
        let debug = format!("{token:?}");
        assert!(!debug.contains("tok-abcdef123456"), "{debug}");
        assert!(debug.contains("Bearer"), "{debug}");
    }
}
