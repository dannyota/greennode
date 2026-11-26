//! Read GreenNode network resources.

mod bandwidth;
mod connectivity;
mod nat;
mod route_inventory;
mod routes;
mod virtual_ip;
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

pub use vpc::*;

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
