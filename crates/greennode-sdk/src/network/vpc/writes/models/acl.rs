use serde::Serialize;

use crate::network::NetworkAcl;

/// Selects a VPC and names a new ACL.
#[derive(Clone)]
#[non_exhaustive]
pub struct CreateNetworkAclInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
}

impl CreateNetworkAclInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
        }
    }
}

/// The ACL returned by the service.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateNetworkAclOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
}

/// Selects the unattached, nondefault ACL to delete.
#[derive(Clone)]
#[non_exhaustive]
pub struct DeleteNetworkAclInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
}

impl DeleteNetworkAclInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
        }
    }
}

/// An accepted ACL deletion has no response fields.
#[derive(Clone, PartialEq, Serialize)]
pub struct DeleteNetworkAclOutput {}

/// Selects an ACL and defines a traffic rule.
#[derive(Clone)]
#[non_exhaustive]
pub struct AddNetworkAclRuleInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Direction of traffic.
    pub direction: String,
    /// Rule sequence number.
    pub priority: i64,
    /// Traffic protocol.
    pub protocol: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Action applied to matching traffic.
    pub action: String,
    /// Lower bound of the port range.
    pub port_range_min: i64,
    /// Upper bound of the port range.
    pub port_range_max: i64,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AddNetworkAclRuleInput {
    /// Sets the required operation inputs.
    pub fn new(
        network_acl_id: impl Into<String>,
        direction: impl Into<String>,
        priority: i64,
        protocol: impl Into<String>,
        cidr: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            direction: direction.into(),
            priority,
            protocol: protocol.into(),
            cidr: cidr.into(),
            action: action.into(),
            port_range_min: 0,
            port_range_max: 0,
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether a rule was added.
#[derive(Clone, PartialEq, Serialize)]
pub struct AddNetworkAclRuleOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects a rule by ACL, direction and priority.
#[derive(Clone)]
#[non_exhaustive]
pub struct RemoveNetworkAclRuleInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Direction of traffic.
    pub direction: String,
    /// Rule sequence number.
    pub priority: i64,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl RemoveNetworkAclRuleInput {
    /// Sets the required operation inputs.
    pub fn new(
        network_acl_id: impl Into<String>,
        direction: impl Into<String>,
        priority: i64,
    ) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            direction: direction.into(),
            priority,
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether a rule was removed.
#[derive(Clone, PartialEq, Serialize)]
pub struct RemoveNetworkAclRuleOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects the ACL and the subnet to associate.
#[derive(Clone)]
#[non_exhaustive]
pub struct AssociateNetworkAclSubnetInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AssociateNetworkAclSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// The resulting ACL and identity of the prior ACL if the subnet moved.
#[derive(Clone, PartialEq, Serialize)]
pub struct AssociateNetworkAclSubnetOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
    /// Identifier of the previous ACL, when an association moved.
    #[serde(rename = "PreviousNetworkACLID")]
    pub previous_network_acl_id: String,
}

/// Selects the ACL and the subnet to disassociate.
#[derive(Clone)]
#[non_exhaustive]
pub struct DisassociateNetworkAclSubnetInput {
    /// Identifier of the network ACL.
    pub network_acl_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DisassociateNetworkAclSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(network_acl_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            network_acl_id: network_acl_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// The resulting ACL and whether its subnet association changed.
#[derive(Clone, PartialEq, Serialize)]
pub struct DisassociateNetworkAclSubnetOutput {
    /// The ACL returned by the operation.
    #[serde(rename = "ACL")]
    pub acl: NetworkAcl,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

crate::wire::product::output::public_fields!(CreateNetworkAclOutput { acl });
crate::wire::product::output::public_fields!(DeleteNetworkAclOutput {});
crate::wire::product::output::public_fields!(AddNetworkAclRuleOutput { acl, changed });
crate::wire::product::output::public_fields!(RemoveNetworkAclRuleOutput { acl, changed });
crate::wire::product::output::public_fields!(AssociateNetworkAclSubnetOutput {
    acl,
    changed,
    previous_network_acl_id
});
crate::wire::product::output::public_fields!(DisassociateNetworkAclSubnetOutput { acl, changed });

crate::error::debug_record!(CreateNetworkAclInput { vpc_id, name });
crate::error::debug_record!(DeleteNetworkAclInput { network_acl_id });
crate::error::debug_record!(AddNetworkAclRuleInput {
    network_acl_id,
    direction,
    priority,
    protocol,
    cidr,
    action,
    port_range_min,
    port_range_max,
    no_wait
});
crate::error::debug_record!(RemoveNetworkAclRuleInput {
    network_acl_id,
    direction,
    priority,
    no_wait
});
crate::error::debug_record!(AssociateNetworkAclSubnetInput {
    network_acl_id,
    subnet_id,
    no_wait
});
crate::error::debug_record!(DisassociateNetworkAclSubnetInput {
    network_acl_id,
    subnet_id,
    no_wait
});
