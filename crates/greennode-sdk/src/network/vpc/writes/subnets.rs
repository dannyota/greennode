use http::{Method, StatusCode};
use serde_json::json;

use crate::network::Subnet;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::super::Client;
use super::models::*;
use super::reads::subnet_created;
use super::wire::{CREATE_BOUND, INTERVAL, Request, Write, ipv4_prefix, is_5xx, not_sent};

const CREATE: Request = Request::new(
    "network.CreateSubnet",
    "v2/{}/networks/{}/subnets",
    &Method::POST,
    &[StatusCode::OK],
);
const UPDATE: Request = Request::new(
    "network.UpdateSubnet",
    "v2/{}/networks/{}/subnets/{}",
    &Method::PATCH,
    &[StatusCode::OK],
)
.repeatable();
const DELETE: Request = Request::new(
    "network.DeleteSubnet",
    "v2/{}/networks/{}/subnets/{}",
    &Method::DELETE,
    &[StatusCode::OK],
)
.repeatable();
const CREATE_ADVICE: &str =
    "the subnet may exist, list the VPC's subnets and match by CIDR before creating it again";

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a subnet and optionally waits for it to become active.
    pub async fn create_subnet(
        &self,
        ctx: &RequestContext,
        input: CreateSubnetInput,
    ) -> Result<CreateSubnetOutput, Failure<CreateSubnetOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            let vpc_id = &input.vpc_id;
            let op = CREATE.id;
            write.check(
                op,
                &[
                    ("ZoneID", &input.zone_id),
                    ("Name", &input.name),
                    ("CIDR", &input.cidr),
                ],
                &[("VPCID", vpc_id)],
            )?;
            ipv4_prefix(op, &input.cidr)?;
            let reply = write
                .send(
                    CREATE,
                    &[vpc_id],
                    Some(json!({"name": input.name, "cidr": input.cidr, "zoneId": input.zone_id})),
                )
                .await
                .map_err(|error| write.advise(error, CREATE_ADVICE))?;
            let mut subnet = write.created_with(
                CREATE,
                reply,
                (true, !input.no_wait),
                |row: &Subnet| &row.uuid,
                CREATE_ADVICE,
                subnet_created,
            )?;
            if input.no_wait {
                return Ok(CreateSubnetOutput { subnet });
            }
            let id = subnet.uuid.clone();
            let resource = format!("subnet {id}");
            let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{op}: {resource} did not reach ACTIVE within 3m0s; the subnet exists and this create must not be repeated"),
            async |write, ctx| {
                subnet = match write.subnet(ctx, vpc_id, &id).await {
                    Ok(row) => row,
                    Err(error) if crate::confirms_absence(&error, "network.GetSubnet", "GET", "v2/{}/networks/{}/subnets/{}") => return Ok(None),
                    Err(error) => return Err(write.settled_error(&format!("{op}: {resource}"), error)),
                };
                match subnet.status.as_str() {
                    "ACTIVE" => Ok(Some(())),
                    "ERROR" => Err(write.status_error(op, &resource)),
                    _ => Ok(None),
                }
            }).await;
            write.outcome(op, subnet, result, |subnet| CreateSubnetOutput { subnet })
        })
    }

    /// Renames a subnet with no secondary subnets and reads the result.
    pub async fn update_subnet(
        &self,
        ctx: &RequestContext,
        input: UpdateSubnetInput,
    ) -> Result<UpdateSubnetOutput, Failure<UpdateSubnetOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            let (vpc, id) = (&input.vpc_id, &input.subnet_id);
            let op = UPDATE.id;
            write.check(
                op,
                &[("Name", &input.name)],
                &[("VPCID", vpc), ("SubnetID", id)],
            )?;
            let mut subnet = write.subnet(ctx, vpc, id).await.map_err(not_sent)?;
            if !subnet.secondary_subnets.is_empty() {
                return Err(not_sent(write.local(ErrorClass::InvalidInput, &format!("vngcloud: invalid input: {op}: subnet {id} has {} secondary subnet(s); rename is refused until the rename body's effect on them is confirmed live", subnet.secondary_subnets.len()))));
            }
            let _ = write
                .send(UPDATE, &[vpc, id], Some(json!({"name": input.name})))
                .await?;
            match write.renamed_subnet(vpc, id).await {
                Ok(subnet) => Ok(UpdateSubnetOutput { subnet }),
                Err(error) => {
                    subnet.name = input.name;
                    let error = write.settled_error(&format!("{op}: subnet {id}"), error);
                    write.outcome(op, subnet, Err(error), |subnet| UpdateSubnetOutput {
                        subnet,
                    })
                }
            }
        })
    }

    /// Deletes a subnet after checking that no servers, interfaces, VIPs or ACLs use it.
    pub async fn delete_subnet(
        &self,
        ctx: &RequestContext,
        input: DeleteSubnetInput,
    ) -> Result<DeleteSubnetOutput, Failure<DeleteSubnetOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            let (vpc, id) = (&input.vpc_id, &input.subnet_id);
            let op = DELETE.id;
            write.check(op, &[], &[("VPCID", vpc), ("SubnetID", id)])?;
            let current = write.subnet(ctx, vpc, id).await.map_err(not_sent)?;
            if current.status == "DELETED" {
                return Err(not_sent(write.local(
                    ErrorClass::NotFound,
                    &format!("vngcloud: resource not found: {op}: subnet {id} is already DELETED"),
                )));
            }
            if !current.network_id.is_empty() && current.network_id != *vpc {
                return Err(not_sent(write.local(
                    ErrorClass::InvalidInput,
                    &format!(
                        "vngcloud: invalid input: {op}: subnet {id} belongs to VPC {}, not {vpc}",
                        current.network_id
                    ),
                )));
            }
            write.subnet_unused(vpc, id).await.map_err(not_sent)?;
            if let Err(failure) = write
                .send::<DeleteSubnetOutput>(DELETE, &[vpc, id], None)
                .await
            {
                let error = failure.error();
                let network = error.class() == ErrorClass::Api
                    && error.http_status().is_none()
                    && error
                        .api_error()
                        .is_some_and(|api| api.observed_status().is_none());
                if failure.write_dispatch() == WriteDispatch::MayHaveBeenSent
                    && super::wire::can_confirm(error)
                    && (network || is_5xx(error))
                    && write
                        .subnets(ctx, vpc)
                        .await
                        .is_ok_and(|rows| rows.iter().all(|row| row.uuid != *id))
                {
                    return Ok(DeleteSubnetOutput {});
                }
                let (error, _, dispatch) = failure.into_parts();
                return Err(Failure::new(write.screen(error), None, dispatch));
            }
            if input.no_wait {
                return Ok(DeleteSubnetOutput {});
            }
            let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{op}: subnet {id} still listed under VPC {vpc} after 3m0s; delete was sent and a rerun is safe"),
            async |write, ctx| {
                let rows = write.subnets(ctx, vpc).await.map_err(|error| write.settled_error(&format!("{op}: subnet {id}"), error))?;
                match rows.into_iter().find(|row| row.uuid == *id) {
                    None => Ok(Some(())),
                    Some(row) if row.status == "ERROR" => Err(write.status_error(op, &format!("subnet {id}"))),
                    Some(_) => Ok(None),
                }
            }).await;
            write.outcome(op, (), result, |()| DeleteSubnetOutput {})
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    async fn subnet_unused(&mut self, vpc: &str, id: &str) -> Result<(), Error> {
        let op = DELETE.id;
        let servers = self.servers(id).await?;
        if servers > 0 {
            return Err(self.in_use(&format!(
                "{op}: subnet {id} has {servers} server(s) attached"
            )));
        }
        let interfaces = self.interfaces(id).await?;
        if interfaces > 0 {
            return Err(self.in_use(&format!(
                "{op}: subnet {id} has {interfaces} network interface(s) attached"
            )));
        }
        let vips = self.virtual_ips(id).await?;
        if vips > 0 {
            return Err(self.in_use(&format!(
                "{op}: subnet {id} has {vips} virtual IP(s) attached"
            )));
        }
        let ctx = self.scoped(super::super::wire::walk_deadline());
        let acls = self.all_acls(&ctx).await.map_err(|error| {
            self.local(
                ErrorClass::Api,
                &format!(
                    "{op}: could not confirm no network ACL in VPC {vpc} still holds subnet {id}"
                ),
            )
            .with_cause(error)
        })?;
        for acl in acls
            .into_iter()
            .filter(|acl| acl.network_id.is_empty() || acl.network_id == vpc)
        {
            let detail =
                match self.acl(self.ctx, &acl.uuid).await {
                    Ok(detail) => detail,
                    Err(error)
                        if crate::confirms_absence(
                            &error,
                            "network.GetNetworkACL",
                            "GET",
                            "v2/{}/network-acl/{}",
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(self
                        .local(
                            ErrorClass::Api,
                            &format!(
                                "{op}: could not confirm network ACL {} does not hold subnet {id}",
                                acl.uuid
                            ),
                        )
                        .with_cause(error)),
                };
            if detail.vpc_id == vpc && detail.subnet_ids.iter().any(|held| held == id) {
                let name = if detail.name.is_empty() {
                    &acl.uuid
                } else {
                    &detail.name
                };
                return Err(self.in_use(&format!(
                    "{op}: subnet {id} is associated with network ACL {name}; disassociate it first"
                )));
            }
        }
        Ok(())
    }
}
