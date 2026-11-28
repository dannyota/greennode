use std::time::Duration;

use http::{Method, StatusCode};
use serde_json::json;

use crate::{ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::super::Client;
use super::models::*;
use super::wire::{
    CREATE_BOUND, INTERVAL, NetworkWriteError, Request, Write, ipv4_prefix, is_4xx, not_sent,
};

const CREATE: Request = Request::new(
    "network.CreateVPC",
    "v2/{}/networks",
    &Method::POST,
    &[StatusCode::OK],
);
const UPDATE: Request = Request::new(
    "network.UpdateVPC",
    "v2/{}/networks/{}",
    &Method::PATCH,
    &[StatusCode::OK],
)
.repeatable();
const DELETE: Request = Request::new(
    "network.DeleteVPC",
    "v2/{}/networks/{}",
    &Method::DELETE,
    &[StatusCode::OK],
)
.repeatable();
const ENABLE_DNS: Request = Request::new(
    "network.EnableVPCPrivateDNS",
    "v2/{}/networks/{}/enableDns",
    &Method::PATCH,
    &[StatusCode::OK],
)
.once();
const CREATE_ADVICE: &str =
    "the VPC may exist, list vpcs and match the name exactly before creating it again";

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a VPC and optionally waits for it to become active.
    pub async fn create_vpc(
        &self,
        ctx: &RequestContext,
        input: CreateVpcInput,
    ) -> Result<CreateVpcOutput, Failure<CreateVpcOutput>> {
        let mut write = Write::new(self, ctx)?;
        write.check(
            CREATE.id,
            &[("Name", &input.name), ("CIDR", &input.cidr)],
            &[],
        )?;
        ipv4_prefix(CREATE.id, &input.cidr)?;
        let reply = write
            .send(
                CREATE,
                &[],
                Some(json!({"name": input.name, "cidr": input.cidr})),
            )
            .await
            .map_err(|error| write.advise(error, CREATE_ADVICE))?;
        let mut vpc = write.created(
            CREATE,
            reply,
            true,
            |row: &super::super::models::Vpc| &row.uuid,
            !input.no_wait,
            CREATE_ADVICE,
        )?;
        if input.no_wait {
            return Ok(CreateVpcOutput { vpc });
        }
        let id = vpc.uuid.clone();
        let resource = format!("VPC {id}");
        let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{}: {resource} did not reach ACTIVE within 3m0s; the VPC exists and this create must not be repeated", CREATE.id),
            async |write, ctx| {
                vpc = match write.vpc(ctx, &id).await {
                    Ok(row) => row,
                    Err(error) if error.has_class(ErrorClass::NotFound) => return Ok(None),
                    Err(error) => return Err(write.settled_error(&format!("{}: {resource}", CREATE.id), error)),
                };
                match vpc.status.as_str() {
                    "ACTIVE" => Ok(Some(())),
                    "ERROR" => Err(write.status_error(CREATE.id, &resource)),
                    _ => Ok(None),
                }
            }).await;
        write.outcome(CREATE.id, vpc, result, |vpc| CreateVpcOutput { vpc })
    }

    /// Renames a VPC and reads the result after the patch.
    pub async fn update_vpc(
        &self,
        ctx: &RequestContext,
        input: UpdateVpcInput,
    ) -> Result<UpdateVpcOutput, Failure<UpdateVpcOutput>> {
        let mut write = Write::new(self, ctx)?;
        let id = &input.vpc_id;
        write.check(UPDATE.id, &[("Name", &input.name)], &[("VPCID", id)])?;
        write
            .send(UPDATE, &[id], Some(json!({"name": input.name})))
            .await?;
        match write.renamed_vpc(id).await {
            Ok(vpc) => Ok(UpdateVpcOutput { vpc }),
            Err(error) => {
                let vpc = super::super::models::Vpc {
                    uuid: input.vpc_id,
                    name: input.name,
                    ..Default::default()
                };
                let error = write.settled_error(&format!("{}: VPC {}", UPDATE.id, vpc.uuid), error);
                write.outcome(UPDATE.id, vpc, Err(error), |vpc| UpdateVpcOutput { vpc })
            }
        }
    }

    /// Deletes an unused VPC and optionally waits until it is absent.
    pub async fn delete_vpc(
        &self,
        ctx: &RequestContext,
        input: DeleteVpcInput,
    ) -> Result<DeleteVpcOutput, Failure<DeleteVpcOutput>> {
        let mut write = Write::new(self, ctx)?;
        let id = &input.vpc_id;
        let resource = format!("VPC {id}");
        write.check(DELETE.id, &[], &[("VPCID", id)])?;
        let row = write.vpc(ctx, id).await.map_err(not_sent)?;
        if row.server_count > 0 || row.volume_count > 0 {
            let message = format!(
                "{}: {resource} has {} server(s) and {} volume(s); delete them first",
                DELETE.id, row.server_count, row.volume_count
            );
            return Err(not_sent(write.in_use(&message)));
        }
        let subnets = write.subnets(ctx, id).await.map_err(not_sent)?;
        if !subnets.is_empty() {
            let message = format!(
                "{}: {resource} has {} subnet(s); delete them first",
                DELETE.id,
                subnets.len()
            );
            return Err(not_sent(write.in_use(&message)));
        }
        write.send(DELETE, &[id], None).await.map_err(|failure: Failure<DeleteVpcOutput>| {
            if failure.error().api_error().is_some_and(|api| api.message().to_ascii_lowercase().contains("contains the subnet")) {
                let (error, _, dispatch) = failure.into_parts();
                let message = format!("{}: {resource}: {}; a subnet deleted in the last 15 minutes can still block this delete, a rerun is safe",
                    DELETE.id, error.api_error().map_or("", crate::ApiError::message));
                Failure::new(write.in_use(&message).with_cause(error), None, dispatch)
            } else { failure }
        })?;
        if input.no_wait {
            return Ok(DeleteVpcOutput {});
        }
        let result = write.settle(CREATE_BOUND, INTERVAL,
            &format!("{}: {resource} did not reach 404 within 3m0s; delete was sent and a rerun is safe", DELETE.id),
            async |write, ctx| match write.vpc(ctx, id).await {
                Ok(row) if row.status == "ERROR" => Err(write.status_error(DELETE.id, &resource)),
                Ok(_) => Ok(None),
                Err(error) if error.has_class(ErrorClass::NotFound) => Ok(Some(())),
                Err(error) => Err(write.settled_error(&format!("{}: {resource}", DELETE.id), error)),
            }).await;
        write.outcome(DELETE.id, (), result, |()| DeleteVpcOutput {})
    }

    /// Enables private DNS, or waits for an enable that is already in progress.
    pub async fn enable_vpc_private_dns(
        &self,
        ctx: &RequestContext,
        input: EnableVpcPrivateDnsInput,
    ) -> Result<EnableVpcPrivateDnsOutput, Failure<EnableVpcPrivateDnsOutput>> {
        let mut write = Write::new(self, ctx)?;
        let id = &input.vpc_id;
        let op = ENABLE_DNS.id;
        let resource = format!("VPC {id}");
        write.check(op, &[], &[("VPCID", id)])?;
        let mut vpc = write.vpc(ctx, id).await.map_err(not_sent)?;
        let mut dispatch = WriteDispatch::NotSent;
        match vpc.dns_status.as_str() {
            "ENABLED" => {
                return write.unchanged(
                    op,
                    EnableVpcPrivateDnsOutput {
                        vpc,
                        changed: false,
                    },
                );
            }
            "ENABLING" => {}
            "DISABLED" => match write.send(ENABLE_DNS, &[id], None).await {
                Ok(_) => dispatch = WriteDispatch::MayHaveBeenSent,
                Err(error)
                    if is_4xx(error.error())
                        || error.write_dispatch() == WriteDispatch::NotSent =>
                {
                    return Err(error);
                }
                Err(error) => {
                    let (error, _, _) = error.into_parts();
                    let error = write.settled_error(&format!("{op}: {resource}"), error);
                    return write.outcome(op, vpc, Err(error), |vpc| EnableVpcPrivateDnsOutput {
                        vpc,
                        changed: false,
                    });
                }
            },
            _ => {
                return Err(not_sent(write.guard_error(
                    NetworkWriteError::UnexpectedStatus,
                    &format!("{op}: {resource} dnsStatus is {:?}", vpc.dns_status),
                )));
            }
        }
        if input.no_wait {
            let out = EnableVpcPrivateDnsOutput { vpc, changed: true };
            return write
                .safe(op, &out)
                .map(|()| out)
                .map_err(|err| Failure::new(err, None, dispatch));
        }
        let result = write.settle(Duration::from_secs(600), Duration::from_secs(10),
            &format!("{op}: {resource} dnsStatus did not reach ENABLED within 10m0s; the enable was sent, rerun EnableVPCPrivateDNS to check again"),
            async |write, ctx| {
                vpc = write.vpc(ctx, id).await.map_err(|error| write.settled_error(&format!("{op}: {resource}"), error))?;
                match vpc.dns_status.as_str() {
                    "ENABLED" => Ok(Some(())),
                    "ENABLING" => Ok(None),
                    _ => Err(write.guard_error(NetworkWriteError::UnexpectedStatus, &format!("{op}: {resource} dnsStatus is {:?}", vpc.dns_status))),
                }
            }).await;
        write
            .outcome(op, vpc, result, |vpc| EnableVpcPrivateDnsOutput {
                vpc,
                changed: true,
            })
            .map_err(|failure| {
                let (error, partial, _) = failure.into_parts();
                Failure::new(error, partial, dispatch)
            })
    }
}
