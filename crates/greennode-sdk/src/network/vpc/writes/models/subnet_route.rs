use serde::Serialize;

use crate::network::{RouteTable, Subnet};

/// Parent VPC, zone, name and address range for a new subnet.
#[derive(Clone)]
#[non_exhaustive]
pub struct CreateSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Availability zone identifier.
    pub zone_id: String,
    /// Resource display name.
    pub name: String,
    /// Address range in CIDR notation.
    pub cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(
        vpc_id: impl Into<String>,
        zone_id: impl Into<String>,
        name: impl Into<String>,
        cidr: impl Into<String>,
    ) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            zone_id: zone_id.into(),
            name: name.into(),
            cidr: cidr.into(),
            no_wait: false,
        }
    }
}

/// The created subnet or its last observed state.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateSubnetOutput {
    /// The subnet returned by the operation.
    #[serde(rename = "Subnet")]
    pub subnet: Subnet,
}

/// Selects a subnet and its replacement name.
#[derive(Clone)]
#[non_exhaustive]
pub struct UpdateSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Resource display name.
    pub name: String,
}

impl UpdateSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(
        vpc_id: impl Into<String>,
        subnet_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            name: name.into(),
        }
    }
}

/// The renamed subnet or submitted name if the read fails.
#[derive(Clone, PartialEq, Serialize)]
pub struct UpdateSubnetOutput {
    /// The subnet returned by the operation.
    #[serde(rename = "Subnet")]
    pub subnet: Subnet,
}

/// Selects a subnet and whether to wait for its removal.
#[derive(Clone)]
#[non_exhaustive]
pub struct DeleteSubnetInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Identifier of the subnet.
    pub subnet_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteSubnetInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, subnet_id: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted subnet deletion has no response fields.
#[derive(Clone, PartialEq, Serialize)]
pub struct DeleteSubnetOutput {}

/// Selects a VPC and names a new route table.
#[derive(Clone)]
#[non_exhaustive]
pub struct CreateRouteTableInput {
    /// Identifier of the VPC.
    pub vpc_id: String,
    /// Resource display name.
    pub name: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl CreateRouteTableInput {
    /// Sets the required operation inputs.
    pub fn new(vpc_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            vpc_id: vpc_id.into(),
            name: name.into(),
            no_wait: false,
        }
    }
}

/// The created table or its last observed state.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateRouteTableOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
}

/// Selects a route table and whether to wait for its removal.
#[derive(Clone)]
#[non_exhaustive]
pub struct DeleteRouteTableInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl DeleteRouteTableInput {
    /// Sets the required operation inputs.
    pub fn new(route_table_id: impl Into<String>) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            no_wait: false,
        }
    }
}

/// An accepted route table deletion has no response fields.
#[derive(Clone, PartialEq, Serialize)]
pub struct DeleteRouteTableOutput {}

/// Selects a table and defines the destination prefix and next hop.
#[derive(Clone)]
#[non_exhaustive]
pub struct AddRouteInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Destination prefix in CIDR notation.
    pub destination_cidr: String,
    /// Next-hop IP address.
    pub target: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl AddRouteInput {
    /// Sets the required operation inputs.
    pub fn new(
        route_table_id: impl Into<String>,
        destination_cidr: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            destination_cidr: destination_cidr.into(),
            target: target.into(),
            no_wait: false,
        }
    }
}

/// The resulting route table and whether a route was added.
#[derive(Clone, PartialEq, Serialize)]
pub struct AddRouteOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// Selects a table and the destination prefix to remove.
#[derive(Clone)]
#[non_exhaustive]
pub struct RemoveRouteInput {
    /// Identifier of the route table.
    pub route_table_id: String,
    /// Destination prefix in CIDR notation.
    pub destination_cidr: String,
    /// Returns after the write is accepted, without waiting for settlement.
    pub no_wait: bool,
}

impl RemoveRouteInput {
    /// Sets the required operation inputs.
    pub fn new(route_table_id: impl Into<String>, destination_cidr: impl Into<String>) -> Self {
        Self {
            route_table_id: route_table_id.into(),
            destination_cidr: destination_cidr.into(),
            no_wait: false,
        }
    }
}

/// The resulting route table and whether a route was removed.
#[derive(Clone, PartialEq, Serialize)]
pub struct RemoveRouteOutput {
    /// The route table returned by the operation.
    #[serde(rename = "RouteTable")]
    pub route_table: RouteTable,
    /// Whether the operation changed the resource.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

crate::wire::product::output::public_fields!(CreateSubnetOutput { subnet });
crate::wire::product::output::public_fields!(UpdateSubnetOutput { subnet });
crate::wire::product::output::public_fields!(DeleteSubnetOutput {});
crate::wire::product::output::public_fields!(CreateRouteTableOutput { route_table });
crate::wire::product::output::public_fields!(DeleteRouteTableOutput {});
crate::wire::product::output::public_fields!(AddRouteOutput {
    route_table,
    changed
});
crate::wire::product::output::public_fields!(RemoveRouteOutput {
    route_table,
    changed
});

crate::error::debug_record!(CreateSubnetInput {
    vpc_id,
    zone_id,
    name,
    cidr,
    no_wait
});
crate::error::debug_record!(UpdateSubnetInput {
    vpc_id,
    subnet_id,
    name
});
crate::error::debug_record!(DeleteSubnetInput {
    vpc_id,
    subnet_id,
    no_wait
});
crate::error::debug_record!(CreateRouteTableInput {
    vpc_id,
    name,
    no_wait
});
crate::error::debug_record!(DeleteRouteTableInput {
    route_table_id,
    no_wait
});
crate::error::debug_record!(AddRouteInput {
    route_table_id,
    destination_cidr,
    target,
    no_wait
});
crate::error::debug_record!(RemoveRouteInput {
    route_table_id,
    destination_cidr,
    no_wait
});
