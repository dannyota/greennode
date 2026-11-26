//! Read-only regional price quotes and monthly setup estimates.

mod estimate;
mod get_quote;
mod models;
mod validate;

#[cfg(test)]
mod tests;

pub use crate::model::pricing::{
    DiscoveryFailure, DiscoveryMetadata, DiscoveryPrice, PricingNote, discovery_notes,
};
pub use get_quote::{GetQuoteInput, GetQuoteOutput, PriceProperty};
pub use models::{
    EstimateArithmeticError, EstimateCharge, EstimateCoverage, EstimateInput, EstimateItem,
    EstimateOutput, EstimatePublicIp, EstimateServer, EstimateStorage, EstimateTotal,
    EstimateVolume,
};

use crate::{Config, config::session::Session};
use std::sync::Arc;

/// Snapshot quote resource.
pub const RESOURCE_SNAPSHOT: &str = "snapshot";
/// Public virtual IP quote resource.
pub const RESOURCE_PUBLIC_VIP: &str = "public-vip";
/// Server quote resource.
pub const RESOURCE_SERVER: &str = "server";
/// Volume quote resource.
pub const RESOURCE_VOLUME: &str = "volume";
/// Load balancer quote resource.
pub const RESOURCE_LOAD_BALANCER: &str = "load-balancer";
/// Quote a creation.
pub const ACTION_CREATE: &str = "create";
/// Quote a resize, which can return a credit.
pub const ACTION_RESIZE: &str = "resize";

/// Pricing client sharing the configuration's session.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
}

impl Client {
    /// Creates a pricing client without network I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
        }
    }
}
