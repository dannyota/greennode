//! Read servers and their regional compute resources.

mod aggregates;
mod flavors;
mod inventory;
mod models;
mod mutation;
mod prices;
mod quotes;
mod server_actions;
mod server_groups;
mod server_writes;
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

pub use quotes::{CreateServerInput, ResizeServerInput};
pub use server_actions::*;
pub use server_groups::*;
pub use server_writes::{CreateServerOutput, DeleteServerInput, DeleteServerOutput};

use std::sync::Arc;

use crate::config::Session;
use crate::{Config, Error, Failure, WriteDispatch};

/// Client for servers, their related resources, and the compute catalog.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
    pricing: crate::pricing::Client,
    volume: crate::volume::Client,
}

impl Client {
    /// Shares the configuration's session without making any requests.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
            pricing: crate::pricing::Client::new(config),
            volume: crate::volume::Client::new(config),
        }
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}
