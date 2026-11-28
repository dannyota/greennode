use http::{Method, StatusCode};
use serde::Serialize;

use crate::network::NetworkAcl;
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::super::super::Client;
use super::body::{IdentifierPath, RequestModel};
use super::models::*;
use super::wire::{BOUND, INTERVAL, NetworkWriteError, Request, Write, is_4xx, is_5xx, not_sent};

const CREATE: Request = Request::new(
    "network.CreateNetworkACL",
    "v2/{}/network-acl",
    &Method::POST,
    &[StatusCode::CREATED],
);
const DELETE: Request = Request::new(
    "network.DeleteNetworkACL",
    "v2/{}/network-acl/{}",
    &Method::DELETE,
    &[StatusCode::NO_CONTENT],
)
.once();
const CREATE_ADVICE: &str = "the ACL may exist, list network ACLs by name in this VPC and compare createdAt, since a same-name ACL may already exist";

#[derive(Serialize)]
struct CreateBody<'a> {
    name: &'a str,
    #[serde(rename = "vpc")]
    vpc_id: &'a str,
}

impl RequestModel for CreateBody<'_> {
    const IDENTIFIER_PATHS: &'static [IdentifierPath] = &[&["vpc"]];
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a network ACL in a VPC.
    pub async fn create_network_acl(
        &self,
        ctx: &RequestContext,
        input: CreateNetworkAclInput,
    ) -> Result<CreateNetworkAclOutput, Failure<CreateNetworkAclOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            super::wire::required(CREATE.id, "VPCID", !input.vpc_id.is_empty())?;
            write.check(CREATE.id, &[("Name", &input.name)], &[])?;
            let body = CreateBody {
                name: &input.name,
                vpc_id: &input.vpc_id,
            };
            let reply = write
                .send_model(CREATE, &[], &body)
                .await
                .map_err(|error| write.advise(error, CREATE_ADVICE))?;
            let acl = write.created(
                CREATE,
                reply,
                true,
                |row: &NetworkAcl| &row.uuid,
                false,
                CREATE_ADVICE,
            )?;
            Ok(CreateNetworkAclOutput { acl })
        })
    }

    /// Deletes a nondefault network ACL with no subnet associations.
    pub async fn delete_network_acl(
        &self,
        ctx: &RequestContext,
        input: DeleteNetworkAclInput,
    ) -> Result<DeleteNetworkAclOutput, Failure<DeleteNetworkAclOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            let id = &input.network_acl_id;
            let op = DELETE.id;
            write.check(op, &[], &[("NetworkACLID", id)])?;
            let acl = write.acl(ctx, id).await.map_err(not_sent)?;
            if acl.default_acl {
                return Err(not_sent(write.guard_error(
                    NetworkWriteError::DefaultResource,
                    &format!("{op}: network ACL {id} is a default ACL"),
                )));
            }
            if !acl.subnet_ids.is_empty() {
                return Err(not_sent(write.in_use(&format!(
                    "{op}: network ACL {id} has {} associated subnet(s); disassociate them first",
                    acl.subnet_ids.len()
                ))));
            }
            let Err(failure) = write
                .send::<DeleteNetworkAclOutput>(DELETE, &[id], None)
                .await
            else {
                return Ok(DeleteNetworkAclOutput {});
            };
            let (error, _, dispatch) = failure.into_parts();
            if let Some(busy) = write.acl_busy(&error) {
                return Err(Failure::new(busy.with_cause(error), None, dispatch));
            }
            if dispatch == WriteDispatch::MayHaveBeenSent
                && is_5xx(&error)
                && super::wire::can_confirm(&error)
            {
                let absent = write
                    .settle(
                        BOUND,
                        INTERVAL,
                        &format!("network ACL {id} still listed 1m0s after the delete"),
                        async |write, ctx| {
                            let acls = write.all_acls(ctx).await?;
                            Ok(acls
                                .iter()
                                .all(|acl| acl.uuid != *id && acl.id != *id)
                                .then_some(()))
                        },
                    )
                    .await;
                if absent.is_ok() {
                    return Ok(DeleteNetworkAclOutput {});
                }
            }
            Err(Failure::new(write.screen(error), None, dispatch))
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    fn acl_busy(&self, error: &Error) -> Option<Error> {
        let api = error.api_error()?;
        let message = api.message().to_ascii_lowercase();
        ["is busy doing something", "is being updated"]
            .into_iter()
            .any(|phrase| message.contains(phrase))
            .then(|| self.guard_error(NetworkWriteError::Busy, api.message()))
    }

    pub(super) fn acl_send_failure<O>(
        &self,
        op: &'static str,
        id: &str,
        failure: Failure<O>,
    ) -> Failure<O> {
        let (error, _, dispatch) = failure.into_parts();
        let error = if let Some(busy) = self.acl_busy(&error) {
            busy.with_cause(error)
        } else if dispatch == WriteDispatch::MayHaveBeenSent && !is_4xx(&error) {
            self.settled_error(&format!("{op}: network ACL {id}: the write was sent once and may have reached the server; read the ACL again before retrying"), error)
        } else {
            error
        };
        Failure::new(self.screen(error), None, dispatch)
    }

    pub(super) async fn active_acl(
        &mut self,
        op: &'static str,
        id: &str,
    ) -> Result<NetworkAcl, Error> {
        self.active(
            op,
            &format!("network ACL {id}"),
            async |write, ctx| write.acl(ctx, id).await,
            |row| &row.status,
        )
        .await
    }

    pub(super) async fn settle_acl(
        &mut self,
        op: &'static str,
        id: &str,
        last: &mut Option<NetworkAcl>,
    ) -> Result<(), Error> {
        self.settle(BOUND, INTERVAL,
            &format!("{op}: network ACL {id} did not settle within 1m0s; the write may have reached the server and is safe to run again"),
            async |write, ctx| {
                let row = match write.acl(ctx, id).await {
                    Ok(row) => row,
                    Err(error) if crate::confirms_absence(&error, "network.GetNetworkACL", "GET", "v2/{}/network-acl/{}") => return Ok(None),
                    Err(error) => return Err(write.settled_error(&format!("{op}: network ACL {id}"), error)),
                };
                let active = row.status == "ACTIVE";
                let error = row.status == "ERROR";
                *last = Some(row);
                if error { return Err(write.status_error(op, &format!("network ACL {id}"))); }
                Ok(active.then_some(()))
            }).await
    }
}
