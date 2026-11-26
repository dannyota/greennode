//! Read servers and their regional compute resources.

mod aggregates;
mod flavors;
mod inventory;
mod models;
mod prices;
mod wire;

#[cfg(test)]
mod tests;

pub use crate::model::server::{
    Flavor, Image, NetworkInterface, PackageLimit, Server, ServerSecgroup, Zone,
};
pub use models::*;
pub use prices::{
    FlavorPrice, FlavorPriceReference, ListFlavorPricesInput, ListFlavorPricesOutput,
};

use std::sync::Arc;

use crate::config::Session;
use crate::{Config, Error, Failure, WriteDispatch};

/// Client for servers, their related resources, and the compute catalog.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
}

impl Client {
    /// Shares the configuration's session without making any requests.
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
