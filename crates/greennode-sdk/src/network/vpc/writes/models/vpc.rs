use serde::Serialize;

use crate::network::{DhcpOptions, Vpc};

/// Name, address range and settlement preference for a new VPC.
#[derive(Clone)]
#[non_exhaustive]
pub struct CreateVpcInput {
    /// Resource display name.
    pub name: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateVpcInput {
    /// Sets the required operation inputs.
    pub fn new(name: impl Into<String>, cidr: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cidr: cidr.into(),
            no_wait: false,
        }
    }
}

/// The created VPC or the last observed state.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateVpcOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
}

/// Selects a VPC and its replacement name.
#[derive(Clone)]
#[non_exhaustive]
pub struct UpdateVpcInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
}

impl UpdateVpcInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
        }
    }
}

/// The VPC after a rename, or submitted fields if the read fails.
#[derive(Clone, PartialEq, Serialize)]
pub struct UpdateVpcOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
}

/// Selects a VPC and whether to wait for its removal.
#[derive(Clone)]
#[non_exhaustive]
pub struct DeleteVpcInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteVpcInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted VPC deletion has no response fields.
#[derive(Clone, PartialEq, Serialize)]
pub struct DeleteVpcOutput {}

/// Selects the VPC whose private DNS should be enabled.
#[derive(Clone)]
#[non_exhaustive]
pub struct EnableVpcPrivateDnsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl EnableVpcPrivateDnsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            no_wait: false,
        }
    }
}

/// Private DNS state and whether an enable was needed.
#[derive(Clone, PartialEq, Serialize)]
pub struct EnableVpcPrivateDnsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Settings for a new DHCP options set.
#[derive(Clone)]
#[non_exhaustive]
pub struct CreateDhcpOptionsInput {
    /// Resource display name.
    pub name: String,
    /// IPv4 addresses of the DNS servers.
    pub dns_servers: Vec<String>,
    /// Optional maximum transmission unit.
    pub mtu: Option<i64>,
}

impl CreateDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(name: impl Into<String>, dns_servers: Vec<String>) -> Self {
        Self {
            name: name.into(),
            dns_servers,
            mtu: None,
        }
    }
}

/// The DHCP options set returned by the service.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateDhcpOptionsOutput {
    /// The DHCP options set returned by the operation.
    #[serde(rename = "DHCPOptions")]
    pub dhcp_options: DhcpOptions,
}

/// Selects the unattached DHCP options set to delete.
#[derive(Clone)]
#[non_exhaustive]
pub struct DeleteDhcpOptionsInput {
    /// Identifier of the DHCP options set.
    pub dhcp_options_id: String,
}

impl DeleteDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(dhcp_options_id: impl Into<String>) -> Self {
        Self {
            dhcp_options_id: dhcp_options_id.into(),
        }
    }
}

/// An accepted DHCP options deletion has no response fields.
#[derive(Clone, PartialEq, Serialize)]
pub struct DeleteDhcpOptionsOutput {}

/// Selects a VPC and a DHCP options set to attach.
#[derive(Clone)]
#[non_exhaustive]
pub struct SetVpcDhcpOptionsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the DHCP options set.
    pub dhcp_options_id: String,
}

impl SetVpcDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, dhcp_options_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            dhcp_options_id: dhcp_options_id.into(),
        }
    }
}

/// The VPC and whether its DHCP options attachment changed.
#[derive(Clone, PartialEq, Serialize)]
pub struct SetVpcDhcpOptionsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects the VPC whose DHCP options set should be detached.
#[derive(Clone)]
#[non_exhaustive]
pub struct ClearVpcDhcpOptionsInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
}

impl ClearVpcDhcpOptionsInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
        }
    }
}

/// The VPC and whether its DHCP options attachment changed.
#[derive(Clone, PartialEq, Serialize)]
pub struct ClearVpcDhcpOptionsOutput {
    /// The VPC returned by the operation.
    #[serde(rename = "VPC")]
    pub vpc: Vpc,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

crate::wire::product::output::public_fields!(CreateVpcOutput { vpc });
crate::wire::product::output::public_fields!(UpdateVpcOutput { vpc });
crate::wire::product::output::public_fields!(DeleteVpcOutput {});
crate::wire::product::output::public_fields!(EnableVpcPrivateDnsOutput { vpc, changed });
crate::wire::product::output::public_fields!(CreateDhcpOptionsOutput { dhcp_options });
crate::wire::product::output::public_fields!(DeleteDhcpOptionsOutput {});
crate::wire::product::output::public_fields!(SetVpcDhcpOptionsOutput { vpc, changed });
crate::wire::product::output::public_fields!(ClearVpcDhcpOptionsOutput { vpc, changed });

crate::error::debug_record!(CreateVpcInput {
    name,
    cidr,
    no_wait
});
crate::error::debug_record!(UpdateVpcInput { vpc_id, name });
crate::error::debug_record!(DeleteVpcInput { vpc_id, no_wait });
crate::error::debug_record!(EnableVpcPrivateDnsInput { vpc_id, no_wait });
crate::error::debug_record!(CreateDhcpOptionsInput {
    name,
    dns_servers,
    mtu
});
crate::error::debug_record!(DeleteDhcpOptionsInput { dhcp_options_id });
crate::error::debug_record!(SetVpcDhcpOptionsInput {
    vpc_id,
    dhcp_options_id
});
crate::error::debug_record!(ClearVpcDhcpOptionsInput { vpc_id });
