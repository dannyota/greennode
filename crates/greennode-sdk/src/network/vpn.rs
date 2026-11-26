use http::StatusCode;
use serde_json::value::RawValue;

use crate::endpoint::{Base, vnetwork_regional_gateway};
use crate::model::wire::{go_json, model};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, Failure, PagedList, RequestContext};

use super::route_inventory::{InventoryRead, ResponseKind, field, redact_page};
use super::wire::Fields;
use super::{Client, failure};

const VPN: InventoryRead = InventoryRead::new(
    "network.ListVPNConnections",
    "vnetwork/v1/{}/vpns",
    ResponseKind::Vpn,
);

/// Selects a page of VPN connections.
#[derive(Clone, Debug, Default)]
pub struct ListVpnConnectionsInput {
    /// Page number. Zero uses the first page.
    pub page: i64,
    /// Page size. Zero uses ten connections.
    pub size: i64,
}

/// One page of VPN connections, with inline sites and tunnels.
pub type ListVpnConnectionsOutput = PagedList<VpnConnection>;

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Reads one page of VPN inventory for the selected project in Hanoi or HCM.
    pub async fn list_vpn_connections(
        &self,
        ctx: &RequestContext,
        input: ListVpnConnectionsInput,
    ) -> Result<ListVpnConnectionsOutput, Failure<ListVpnConnectionsOutput>> {
        let mut evidence = self.request_evidence(ctx).map_err(failure)?;
        if input.page < 0 || input.size < 0 {
            return Err(failure(
                VPN.input_error("requires nonnegative Page and Size", &evidence),
            ));
        }
        if let Some(project) = self
            .session
            .project_id_context(ctx)
            .await
            .map_err(|error| failure(*error))?
        {
            VPN.check_id("ProjectID", &project, PathIds::Strict, &evidence)
                .map_err(failure)?;
        }
        let base = self.vpn_base(&evidence).map_err(failure)?;
        let project = self
            .inventory_project(ctx, &[], &mut evidence)
            .await
            .map_err(failure)?;
        VPN.check_id("ProjectID", &project, PathIds::Strict, &evidence)
            .map_err(failure)?;
        let page = input.page.max(1);
        let size = if input.size == 0 { 10 } else { input.size };
        let params = format!(
            r#"{{"search":[{{"field":"any","value":""}}],"sort":{{}},"page":{page},"size":{size}}}"#
        );
        let raw = self
            .read_route_inventory(
                ctx,
                &base,
                VPN,
                &[&project],
                &[("params", &params)],
                &mut evidence,
            )
            .await
            .map_err(failure)?;
        decode_vpn(&go_json(&raw), &evidence).map_err(failure)
    }

    fn vpn_base(&self, evidence: &Evidence) -> Result<Base, Error> {
        let invalid = || VPN.config_error("supports hcm-3 and han-1", evidence);
        if !matches!(self.region.as_str(), "hcm-3" | "han-1") {
            return Err(invalid());
        }
        self.session
            .endpoints()
            .vnetwork
            .as_ref()
            .filter(|base| base.is_explicit_override())
            .and_then(|base| Base::parse(base.configured_url()).ok())
            .or_else(|| vnetwork_regional_gateway(&self.region))
            .ok_or_else(invalid)
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn decode_vpn(text: &str, evidence: &Evidence) -> Result<ListVpnConnectionsOutput, Error> {
    let fields = VPN.fields(text, evidence)?;
    let invalid = |()| VPN.invalid(evidence);
    let success: bool = fields.required("success").map_err(invalid)?;
    if let Some(data) = fields.raw("data").map_err(invalid)?
        && data.get() != "null"
    {
        Vec::<VpnConnection>::check_fields(data).map_err(invalid)?;
    }
    let page = field(&fields, "page").map_err(invalid)?;
    let page_size = field(&fields, "size").map_err(invalid)?;
    let total_page = field::<Option<i64>>(&fields, "totalPage").map_err(invalid)?;
    let total_item = field::<Option<i64>>(&fields, "total").map_err(invalid)?;
    if !success {
        return Err(VPN.vpn_refused(Some(StatusCode::OK), false, evidence));
    }
    let total_page = total_page
        .filter(|value| *value >= 0)
        .ok_or_else(|| invalid(()))?;
    let total_item = total_item
        .filter(|value| *value >= 0)
        .ok_or_else(|| invalid(()))?;
    if page <= 0 || page_size <= 0 {
        return Err(invalid(()));
    }
    let items = if let Some(raw) = fields.raw("data").map_err(invalid)? {
        serde_json::from_str::<Vec<VpnConnection>>(raw.get()).map_err(|_| invalid(()))?
    } else if total_page == 0 && total_item == 0 {
        Vec::new()
    } else {
        return Err(invalid(()));
    };
    if items
        .iter()
        .any(|item| !PathIds::WithUnderscore.allows(&item.uuid))
    {
        return Err(invalid(()));
    }
    let mut output = PagedList {
        items,
        page,
        page_size,
        total_page,
        total_item,
    };
    redact_page(VPN, &mut output, evidence)?;
    Ok(output)
}

trait CheckedField {
    fn check_fields(raw: &RawValue) -> Result<(), ()> {
        if raw.get() == "null" { Err(()) } else { Ok(()) }
    }
}

impl CheckedField for String {}
impl CheckedField for i64 {}
impl CheckedField for bool {}

impl CheckedField for Option<String> {
    fn check_fields(_: &RawValue) -> Result<(), ()> {
        Ok(())
    }
}

impl<T: CheckedField> CheckedField for Vec<T> {
    fn check_fields(raw: &RawValue) -> Result<(), ()> {
        let rows = serde_json::from_str::<Vec<&RawValue>>(raw.get()).map_err(|_| ())?;
        rows.into_iter().try_for_each(T::check_fields)
    }
}

macro_rules! vpn_model {
    ($(#[$meta:meta])* pub struct $name:ident {
        $($(#[$field_meta:meta])* $field:ident: $type:ty => $wire:literal, $go:literal;)*
    }) => {
        model! {
            $(#[$meta])* pub struct $name {
                $($(#[$field_meta])* $field: $type => $wire, $go;)*
            }
        }
        impl CheckedField for $name {
            fn check_fields(raw: &RawValue) -> Result<(), ()> {
                let fields: Fields<'_> = serde_json::from_str(raw.get()).map_err(|_| ())?;
                $(if let Some(raw) = fields.raw($wire)? {
                    <$type>::check_fields(raw)?;
                })*
                Ok(())
            }
        }
    };
}

vpn_model! {
    /// A VPN connection with its inline sites and tunnels.
    pub struct VpnConnection {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// VPN display name.
        vpn_name: String => "vpnName", "VPNName";
        /// VPN package UUID.
        package_uuid: String => "packageUuid", "PackageUUID";
        /// Local network address range.
        local_network_cidr: String => "localNetworkCidr", "LocalNetworkCIDR";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Provider-reported provisioning state.
        status: String => "status", "Status";
        /// Provider-reported billing state.
        billing_status: String => "billingStatus", "BillingStatus";
        /// Network zone UUID.
        zone_uuid: String => "zoneUuid", "ZoneUUID";
        /// Local gateway address, when supplied.
        local_gateway_ip: Option<String> => "localGatewayIp", "LocalGatewayIP";
        /// VPN gateway address, when supplied.
        vpn_gateway_ip: Option<String> => "vpnGatewayIp", "VPNGatewayIP";
        /// Subnet details reported for this connection.
        subnet_detail_model: VpnSubnet => "subnetDetailModel", "SubnetDetailModel";
        /// VPC details reported for this connection.
        vpc_detail_model: VpnVpc => "vpcDetailModel", "VPCDetailModel";
        /// Project identifiers reported for this connection.
        project_detail_model: VpnProject => "projectDetailModel", "ProjectDetailModel";
        /// Package details reported for this connection.
        package_model: VpnPackage => "packageModel", "PackageModel";
        /// Remote sites in provider order.
        vpn_sites: Vec<VpnSite> => "vpnSites", "VPNSites";
    }
}

vpn_model! {
    /// Subnet details reported for a VPN connection.
    pub struct VpnSubnet {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Provider subnet type.
        subnet_type: String => "subnetType", "SubnetType";
        /// Last update time as supplied by the service.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Last synchronization time as supplied by the service.
        last_sync_time: String => "lastSyncTime", "LastSyncTime";
        /// Zone identifier.
        zone_id: String => "zoneId", "ZoneID";
    }
}

vpn_model! {
    /// VPC details reported for a VPN connection.
    pub struct VpnVpc {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Region identifier.
        region_id: String => "regionId", "RegionID";
        /// Project identifier.
        project_id: String => "projectId", "ProjectID";
        /// Last synchronization time as supplied by the service.
        last_sync_time: String => "lastSyncTime", "LastSyncTime";
        /// Provider-reported DNS state.
        dns_status: String => "dnsStatus", "DNSStatus";
    }
}

vpn_model! {
    /// Project identifiers reported for a VPN connection.
    pub struct VpnProject {
        /// Alternate provider identifier.
        id: String => "id", "ID";
        /// Backend project identifier.
        backend_project_id: String => "backendProjectId", "BackendProjectID";
        /// vServer project identifier.
        vserver_project_id: String => "vserverProjectId", "VServerProjectID";
    }
}

vpn_model! {
    /// The package selected for a VPN connection.
    pub struct VpnPackage {
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Package identifier.
        package_id: String => "packageId", "PackageID";
        /// Maximum tunnels for this package.
        tunnel_limit: i64 => "tunnelLimit", "TunnelLimit";
        /// Whether this is the default package.
        default: bool => "default", "Default";
    }
}

vpn_model! {
    /// A remote VPN site and its tunnels.
    pub struct VpnSite {
        /// Remote site gateway address.
        remote_gateway_ip: String => "remoteGatewayIp", "RemoteGatewayIP";
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Remote site display name.
        site_name: String => "siteName", "SiteName";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Phase 1 settings in provider order.
        phase1_configs: Vec<VpnPhase1Config> => "phase1Configs", "Phase1Configs";
        /// Site tunnels in provider order.
        tunnels: Vec<VpnTunnel> => "tunnels", "Tunnels";
    }
}

vpn_model! {
    /// Phase 1 negotiation settings for a VPN site.
    pub struct VpnPhase1Config {
        /// Phase 1 encryption algorithm.
        phase1_algorithm: String => "phase1Algorithm", "Phase1Algorithm";
        /// Phase 1 hash algorithm.
        phase1_hash: String => "phase1Hash", "Phase1Hash";
        /// Phase 1 Diffie-Hellman group.
        phase1_dh_group: String => "phase1DhGroup", "Phase1DHGroup";
        /// Phase 1 IKE lifetime as supplied by the service.
        phase1_ike_lifetime: String => "phase1IkeLifeTime", "Phase1IKELifetime";
    }
}

vpn_model! {
    /// A tunnel and its Phase 2 negotiation settings.
    pub struct VpnTunnel {
        /// UUID of the containing site.
        site_uuid: String => "siteUuid", "SiteUUID";
        /// Tunnel display name.
        tunnel_name: String => "tunnelName", "TunnelName";
        /// Remote network address range.
        remote_network_cidr: String => "remoteNetworkCidr", "RemoteNetworkCIDR";
        /// Provider resource identifier.
        uuid: String => "uuid", "UUID";
        /// Provider-reported state.
        status: String => "status", "Status";
        /// Creation time as supplied by the service.
        created_at: String => "createdAt", "CreatedAt";
        /// Phase 2 settings in provider order.
        phase2_configs: Vec<VpnPhase2Config> => "phase2Configs", "Phase2Configs";
    }
}

vpn_model! {
    /// Phase 2 negotiation settings for a VPN tunnel.
    pub struct VpnPhase2Config {
        /// Phase 2 encryption algorithm.
        phase2_algorithm: String => "phase2Algorithm", "Phase2Algorithm";
        /// Phase 2 hash algorithm.
        phase2_hash: String => "phase2Hash", "Phase2Hash";
        /// Phase 2 Diffie-Hellman group.
        phase2_dh_group: String => "phase2DhGroup", "Phase2DHGroup";
        /// Phase 2 IKE lifetime as supplied by the service.
        phase2_ike_lifetime: String => "phase2IkeLifeTime", "Phase2IKELifetime";
    }
}
