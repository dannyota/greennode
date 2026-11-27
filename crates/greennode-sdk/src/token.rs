use time::{Duration, OffsetDateTime};

use crate::Secret;

/// Bearer credentials and their optional expiration.
#[derive(Clone)]
pub struct Token {
    /// The bearer value; debug and serialized representations are redacted.
    pub access_token: Secret,
    /// Expiration instant, or `None` when unknown.
    pub expires_at: Option<OffsetDateTime>,
}

crate::error::debug_record!(Token {
    access_token,
    expires_at
});

impl Token {
    /// Constructs credentials without validating the value or expiry.
    pub fn new(access_token: Secret, expires_at: Option<OffsetDateTime>) -> Self {
        Self {
            access_token,
            expires_at,
        }
    }

    /// Returns true for an empty token, unknown expiration, or less than 30 seconds remaining.
    pub fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        self.access_token.expose_secret().is_empty()
            || self
                .expires_at
                .is_none_or(|expiry| expiry - now < Duration::seconds(30))
    }
}
