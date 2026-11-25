use time::{Duration, OffsetDateTime};

use crate::Secret;

#[derive(Clone, Debug)]
pub struct Token {
    pub access_token: Secret,
    pub expires_at: Option<OffsetDateTime>,
}

impl Token {
    pub fn new(access_token: Secret, expires_at: Option<OffsetDateTime>) -> Self {
        Self {
            access_token,
            expires_at,
        }
    }

    pub fn needs_refresh(&self, now: OffsetDateTime) -> bool {
        self.access_token.expose_secret().is_empty()
            || self
                .expires_at
                .is_none_or(|expiry| expiry - now < Duration::seconds(30))
    }
}
