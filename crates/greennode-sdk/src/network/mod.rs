//! Read GreenNode network resources.

mod bandwidth;
mod connectivity;
mod endpoint_models;
mod endpoint_quote;
mod endpoint_quote_scope;
mod endpoint_writes;
mod endpoints;
mod inventory;
mod mutations;
mod nat;
mod nat_catalog;
mod nat_models;
mod nat_quote;
mod nat_writes;
mod prices;
mod route_inventory;
mod routes;
mod security_group_writes;
mod server_reads;
mod virtual_ip;
mod virtual_ip_writes;
mod vpc;
mod vpn;
mod wire;

pub use crate::model::server::Zone;
pub use bandwidth::{BandwidthPackage, ListBandwidthPackagesInput, ListBandwidthPackagesOutput};
pub use connectivity::{
    ElasticNetworkInterface, Interconnect, ListInterconnectsInput, ListInterconnectsOutput,
    ListNetworkInterfacesInput, ListNetworkInterfacesOutput, ListPeeringsInput, ListPeeringsOutput,
    ListWanIpsInput, ListWanIpsOutput, Peering, WanIp,
};
pub use endpoint_models::{Endpoint, EndpointTag, VNetworkRegion};
pub use endpoint_quote::*;
pub use endpoint_writes::*;
pub use endpoints::*;
pub use nat::{
    GetNatInstanceInput, GetNatInstanceOutput, ListNatInboundRulesInput, ListNatInboundRulesOutput,
    NatDetail, NatInboundRule,
};
pub use routes::{
    GetRouteTableInput, GetRouteTableOutput, ListRouteTableRoutesInput, ListRouteTableRoutesOutput,
    ListRouteTablesInput, ListRouteTablesOutput, Route, RouteTable, RouteTableRoute,
};
pub use virtual_ip::{
    AddressPair, GetVirtualIpAddressInput, GetVirtualIpAddressOutput,
    ListAddressPairsByVirtualIpAddressInput, ListAddressPairsByVirtualIpAddressOutput,
    ListAddressPairsByVirtualSubnetInput, ListAddressPairsByVirtualSubnetOutput,
    ListAllVirtualIpAddressAddressPairsInput, ListAllVirtualIpAddressAddressPairsOutput,
    ListVirtualIpAddressesInput, ListVirtualIpAddressesOutput, VirtualIpAddress,
};
pub use vpn::{
    ListVpnConnectionsInput, ListVpnConnectionsOutput, VpnConnection, VpnPackage, VpnPhase1Config,
    VpnPhase2Config, VpnProject, VpnSite, VpnSubnet, VpnTunnel, VpnVpc,
};

pub use nat_catalog::*;
pub use nat_models::*;
pub use nat_quote::*;
pub use nat_writes::*;
pub use prices::*;
pub use security_group_writes::*;
pub use server_reads::*;
pub use virtual_ip_writes::*;

pub use vpc::*;

use std::sync::{Arc, OnceLock};

use crate::config::Session;
use crate::{Config, Error, Failure, WriteDispatch};

/// Client for network operations.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
    endpoint_zone: Arc<OnceLock<String>>,
    endpoint_verified_zone: Arc<OnceLock<String>>,
}

impl Client {
    /// Shares the configuration's session without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
            endpoint_zone: Arc::new(OnceLock::new()),
            endpoint_verified_zone: Arc::new(OnceLock::new()),
        }
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

#[cfg(test)]
mod tests;
