//! Read GreenNode network resources.

mod bandwidth;
mod nat;
mod wire;

pub use bandwidth::{BandwidthPackage, ListBandwidthPackagesInput, ListBandwidthPackagesOutput};
pub use nat::{
    GetNatInstanceInput, GetNatInstanceOutput, ListNatInboundRulesInput, ListNatInboundRulesOutput,
    NatDetail, NatInboundRule,
};

use std::sync::Arc;

use crate::config::Session;
use crate::{Config, Error, Failure, WriteDispatch};

/// Client for network operations.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
}

impl Client {
    /// Shares the configuration's session without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
        }
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

#[cfg(test)]
mod tests;
