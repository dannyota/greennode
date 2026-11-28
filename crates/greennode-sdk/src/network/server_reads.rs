use std::time::Duration;

use crate::model::wire::{Field, decode_lenient, go_json};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, List, RequestContext};

use super::inventory::{InventoryRead, InventoryStyle, Selection, screen};
use super::wire::Fields;
use super::{Client, failure};

pub use crate::model::server::Server;

const BY_GROUP: InventoryRead = InventoryRead {
    id: "network.ListServersBySecurityGroup",
    template: "v2/{}/secgroups/{}/servers",
    style: InventoryStyle::Server,
};
const BY_SUBNET: InventoryRead = InventoryRead {
    id: "network.ListServersBySubnet",
    template: "v2/{}/servers/subnets/{}",
    style: InventoryStyle::Plain,
};

/// Selects the security group whose server membership to list.
#[derive(Clone)]
pub struct ListServersBySecurityGroupInput {
    /// Security-group identifier.
    pub security_group_id: String,
}

crate::error::debug_record!(ListServersBySecurityGroupInput { security_group_id });

impl ListServersBySecurityGroupInput {
    /// Selects a security group by identifier.
    pub fn new(security_group_id: impl Into<String>) -> Self {
        Self {
            security_group_id: security_group_id.into(),
        }
    }
}

/// Servers using the selected security group.
pub type ListServersBySecurityGroupOutput = List<Server>;

/// Selects the subnet whose attached servers to list.
#[derive(Clone)]
pub struct ListServersBySubnetInput {
    /// Subnet identifier.
    pub subnet_id: String,
}

crate::error::debug_record!(ListServersBySubnetInput { subnet_id });

impl ListServersBySubnetInput {
    /// Selects a subnet by identifier.
    pub fn new(subnet_id: impl Into<String>) -> Self {
        Self {
            subnet_id: subnet_id.into(),
        }
    }
}

/// Servers attached to the selected subnet.
pub type ListServersBySubnetOutput = List<Server>;

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Lists servers using a security group in the selected project.
    pub async fn list_servers_by_security_group(
        &self,
        ctx: &RequestContext,
        input: ListServersBySecurityGroupInput,
    ) -> Result<ListServersBySecurityGroupOutput, Failure<ListServersBySecurityGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            BY_GROUP
                .check_path("SecurityGroupID", &input.security_group_id)
                .map_err(failure)?;
            let ctx = ctx.child_with_deadline(
                self.session.clock().monotonic_now() + Duration::from_secs(30),
            );
            let mut evidence = self.request_evidence(&ctx).map_err(failure)?;
            let body = self
                .server_inventory(&ctx, BY_GROUP, &input.security_group_id, &mut evidence)
                .await
                .map_err(failure)?;
            let text = go_json(&body);
            let fields: Option<Fields<'_>> = BY_GROUP
                .decode(if text.is_empty() { "null" } else { &text }, &evidence)
                .map_err(failure)?;
            let items = fields
                .map(|fields| {
                    decode_lenient(|| {
                        fields.output_raw("data").and_then(|raw| {
                            raw.map(|raw| {
                                serde_json::from_str::<Field<Vec<Server>>>(raw.get())
                                    .map_err(|_| ())
                            })
                            .transpose()
                        })
                    })
                })
                .transpose()
                .map_err(|()| failure(BY_GROUP.shape(&evidence)))?
                .flatten()
                .map_or_else(Vec::new, |field| field.0);
            finish(BY_GROUP, items, &evidence)
        })
    }

    /// Lists servers attached to a subnet in the selected project.
    pub async fn list_servers_by_subnet(
        &self,
        ctx: &RequestContext,
        input: ListServersBySubnetInput,
    ) -> Result<ListServersBySubnetOutput, Failure<ListServersBySubnetOutput>> {
        crate::wire::product::exit::finish!(async {
            BY_SUBNET
                .check_path("SubnetID", &input.subnet_id)
                .map_err(failure)?;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let body = self
                .server_inventory(ctx, BY_SUBNET, &input.subnet_id, &mut evidence)
                .await
                .map_err(failure)?;
            let text = go_json(&body);
            let items: Field<Vec<Server>> = decode_lenient(|| {
                BY_SUBNET.decode(if text.is_empty() { "null" } else { &text }, &evidence)
            })
            .map_err(failure)?;
            finish(BY_SUBNET, items.0, &evidence)
        })
    }

    pub(super) async fn guard_servers_by_security_group(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<ListServersBySecurityGroupOutput, Error> {
        let ctx =
            ctx.child_with_deadline(self.session.clock().monotonic_now() + Duration::from_secs(30));
        let read = InventoryRead {
            style: InventoryStyle::ServerGuard,
            ..BY_GROUP
        };
        let body = self.server_inventory(&ctx, read, id, evidence).await?;
        let text = go_json(&body);
        let fields: Fields<'_> = read.decode(&text, evidence)?;
        let items = fields
            .optional::<Field<Vec<Server>>>("data")
            .map_err(|()| read.shape(evidence))?
            .map_or_else(Vec::new, |field| field.0);
        finish(read, items, evidence).map_err(|failure| failure.into_parts().0)
    }

    #[allow(dead_code)]
    pub(super) async fn guard_servers_by_subnet(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<ListServersBySubnetOutput, Error> {
        let body = self.server_inventory(ctx, BY_SUBNET, id, evidence).await?;
        let text = go_json(&body);
        let items: Field<Vec<Server>> =
            BY_SUBNET.decode(if text.is_empty() { "null" } else { &text }, evidence)?;
        finish(BY_SUBNET, items.0, evidence).map_err(|failure| failure.into_parts().0)
    }

    async fn server_inventory(
        &self,
        ctx: &RequestContext,
        read: InventoryRead,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        if evidence.id_reflects(id) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| screen(*error, evidence))?;
        if evidence.id_reflects(&project) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let base = self
            .session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))?;
        self.inventory(ctx, base, read, Selection::path(&[&project, id]), evidence)
            .await
    }
}

fn server_ids(item: &Server) -> impl Iterator<Item = &str> {
    let interfaces = item
        .internal_interfaces
        .iter()
        .chain(&item.external_interfaces)
        .flatten()
        .flat_map(|nic| {
            [
                &nic.floating_ipid,
                &nic.network_uuid,
                &nic.port_uuid,
                &nic.server_uuid,
                &nic.subnet_uuid,
                &nic.uuid,
            ]
        });
    let groups = item
        .security_groups
        .iter()
        .flatten()
        .map(|group| &group.uuid);
    let tags = item.default_tag_ids.iter().flatten();
    let zones = item.image.flavor_zone_ids.iter().flatten();
    [
        &item.uuid,
        &item.boot_volume_id,
        &item.zone_id,
        &item.flavor_zone_id,
        &item.host_group_id,
        &item.image.id,
        &item.flavor.flavor_id,
        &item.flavor.flavor_zone_id,
        &item.flavor.zone_id,
        &item.zone.uuid,
    ]
    .into_iter()
    .chain(interfaces)
    .chain(groups)
    .chain(tags)
    .chain(zones)
    .map(String::as_str)
    .filter(|id| !id.is_empty())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn finish(
    read: InventoryRead,
    items: Vec<Server>,
    evidence: &Evidence,
) -> Result<List<Server>, Failure<List<Server>>> {
    let protected = InventoryRead {
        style: InventoryStyle::Server,
        ..read
    };
    if items.iter().any(|item| {
        server_ids(item).any(|id| evidence.id_reflects(id))
            || evidence.identifier_output_reflects(&item.server_group_id)
    }) {
        return Err(failure(protected.shape(evidence)));
    }
    if !serde_json::to_value(&items).is_ok_and(|value| !evidence.output_reflects(&value)) {
        return Err(failure(protected.shape(evidence)));
    }
    let items = read.retain(items, evidence).map_err(failure)?;
    Ok(List { items })
}
