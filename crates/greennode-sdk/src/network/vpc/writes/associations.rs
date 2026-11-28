use std::collections::HashMap;

use http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::network::NetworkAcl;
use crate::{ErrorClass, Failure, RequestContext};

use super::super::super::Client;
use super::models::*;
use super::wire::{NetworkWriteError, Request, Write, not_sent, sent};

const ASSOCIATE: Request = Request::new(
    "network.AssociateNetworkACLSubnet",
    "v2/{}/network-acl/{}/subnets",
    &Method::PUT,
    &[StatusCode::OK],
)
.once();
const DISASSOCIATE: Request = Request::new(
    "network.DisassociateNetworkACLSubnet",
    "v2/{}/network-acl/{}/subnets",
    &Method::PUT,
    &[StatusCode::OK],
)
.once();

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Associates a subnet to an ACL and reports its previous ACL, when present.
    pub async fn associate_network_acl_subnet(
        &self,
        ctx: &RequestContext,
        input: AssociateNetworkAclSubnetInput,
    ) -> Result<AssociateNetworkAclSubnetOutput, Failure<AssociateNetworkAclSubnetOutput>> {
        let mut write = Write::new(self, ctx)?;
        let op = ASSOCIATE.id;
        let (id, subnet_id) = (&input.network_acl_id, &input.subnet_id);
        write.check(op, &[], &[("NetworkACLID", id), ("SubnetID", subnet_id)])?;
        let acl = write.active_acl(op, id).await.map_err(not_sent)?;
        if acl.subnet_ids.contains(subnet_id) {
            return write.unchanged(
                op,
                AssociateNetworkAclSubnetOutput {
                    acl,
                    changed: false,
                    previous_network_acl_id: String::new(),
                },
            );
        }
        let subnet = write
            .subnet(ctx, &acl.vpc_id, subnet_id)
            .await
            .map_err(not_sent)?;
        if !subnet.network_id.is_empty() && subnet.network_id != acl.vpc_id {
            return Err(not_sent(write.local(
                ErrorClass::NotFound,
                &format!(
                    "vngcloud: resource not found: {op}: subnet {subnet_id} in VPC {}",
                    acl.vpc_id
                ),
            )));
        }
        let previous = subnet.interface_acl_policy_id;
        let mut subnet_ids = acl.subnet_ids.clone();
        subnet_ids.push(subnet_id.clone());
        let result = write
            .replace_acl_subnets(ASSOCIATE, id, acl, subnet_ids, input.no_wait)
            .await;
        let output = |acl| AssociateNetworkAclSubnetOutput {
            acl,
            changed: true,
            previous_network_acl_id: previous.clone(),
        };
        write.map_result(op, result, output)
    }

    /// Removes a subnet association from an ACL.
    pub async fn disassociate_network_acl_subnet(
        &self,
        ctx: &RequestContext,
        input: DisassociateNetworkAclSubnetInput,
    ) -> Result<DisassociateNetworkAclSubnetOutput, Failure<DisassociateNetworkAclSubnetOutput>>
    {
        let mut write = Write::new(self, ctx)?;
        let op = DISASSOCIATE.id;
        let (id, subnet_id) = (&input.network_acl_id, &input.subnet_id);
        write.check(op, &[], &[("NetworkACLID", id), ("SubnetID", subnet_id)])?;
        let acl = write.active_acl(op, id).await.map_err(not_sent)?;
        if !acl.subnet_ids.contains(subnet_id) {
            return write.unchanged(
                op,
                DisassociateNetworkAclSubnetOutput {
                    acl,
                    changed: false,
                },
            );
        }
        let subnet_ids = acl
            .subnet_ids
            .iter()
            .filter(|id| *id != subnet_id)
            .cloned()
            .collect();
        let output = |acl| DisassociateNetworkAclSubnetOutput { acl, changed: true };
        let result = write
            .replace_acl_subnets(DISASSOCIATE, id, acl, subnet_ids, input.no_wait)
            .await;
        write.map_result(op, result, output)
    }
}

fn subnet_counts(subnets: &[String]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for subnet in subnets {
        *counts.entry(subnet.as_str()).or_default() += 1;
    }
    counts
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    async fn replace_acl_subnets(
        &mut self,
        request: Request,
        id: &str,
        base: NetworkAcl,
        subnets: Vec<String>,
        no_wait: bool,
    ) -> Result<NetworkAcl, Failure<NetworkAcl>> {
        let counts = subnet_counts(&subnets);
        let same = |acl: &NetworkAcl| subnet_counts(&acl.subnet_ids) == counts;
        let unchanged = |before: &NetworkAcl, after: &NetworkAcl| {
            subnet_counts(&before.subnet_ids) == subnet_counts(&after.subnet_ids)
        };
        self.replace_acl(
            request,
            id,
            base,
            json!({"subnetUuids": subnets}),
            no_wait,
            AclChange {
                field: "subnets",
                unchanged,
                matches: same,
                fallback: |mut acl: NetworkAcl| {
                    acl.subnet_ids = subnets.clone();
                    acl
                },
            },
        )
        .await
    }

    pub(super) async fn replace_acl(
        &mut self,
        request: Request,
        id: &str,
        base: NetworkAcl,
        body: Value,
        no_wait: bool,
        change: AclChange<
            impl Fn(&NetworkAcl, &NetworkAcl) -> bool,
            impl Fn(&NetworkAcl) -> bool,
            impl FnOnce(NetworkAcl) -> NetworkAcl,
        >,
    ) -> Result<NetworkAcl, Failure<NetworkAcl>> {
        let op = request.id;
        let recheck = self.acl(self.ctx, id).await.map_err(not_sent)?;
        if !(change.unchanged)(&base, &recheck) {
            return Err(not_sent(self.guard_error(NetworkWriteError::Busy,
                &format!("{op}: network ACL {id} changed since it was read; nothing sent, run the call again"))));
        }
        self.send(request, &[id], Some(body))
            .await
            .map_err(|failure| self.acl_send_failure(op, id, failure))?;
        if no_wait {
            return self.outcome(op, (change.fallback)(base), Ok(()), std::convert::identity);
        }
        let mut last = None;
        let result = self.settle_acl(op, id, &mut last).await;
        let Some(acl) = last else {
            return Err(sent(result.err().unwrap_or_else(|| self.invalid(op)), None));
        };
        let result = result.and_then(|()| {
            if (change.matches)(&acl) { return Ok(()); }
            Err(self.local(ErrorClass::NotSettled, &format!("vngcloud: write outcome not confirmed: {op}: network ACL {id}: {} read after the write do not match what was sent; another writer may have changed the ACL", change.field)))
        });
        self.outcome(op, acl, result, std::convert::identity)
    }
}

pub(super) struct AclChange<U, M, F> {
    pub(super) field: &'static str,
    pub(super) unchanged: U,
    pub(super) matches: M,
    pub(super) fallback: F,
}
