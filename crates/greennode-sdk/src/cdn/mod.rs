//! Public CDN ranges and the vCDN account API.

mod analytics;
mod models;
mod page;
mod reads;
mod wire;

pub use analytics::*;
pub use models::*;
pub use page::{ListIpRangesInput, ListIpRangesOutput};
pub use reads::*;

use std::sync::Arc;

use crate::{Config, Error, ErrorClass, Failure, WriteDispatch, config::Session};

/// A client for the CDN service.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

impl Client {
    /// Creates a CDN client from the configured endpoints and API key.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }
}

/// Additional identities for CDN failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorDetail {
    /// This operation requires a vCDN API key.
    MissingApiKey,
    /// The public IP ranges page did not match the expected format.
    PageFormat,
    /// The CDN is changing state and currently refuses writes.
    ResourceBusy,
    /// A purge must wait for the cooldown since the last purge.
    PurgeCooldown,
}

fn invalid<T>() -> Failure<T> {
    Failure::new(
        Error::new(ErrorClass::InvalidInput),
        None,
        WriteDispatch::NotAWrite,
    )
}
