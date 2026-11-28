use std::net::Ipv4Addr;

use http::{Method, StatusCode};
use serde_json::json;

use crate::network::{DhcpOptions, Vpc};
use crate::{ErrorClass, Failure, RequestContext};

use super::super::super::Client;
use super::models::*;
use super::wire::{BOUND, INTERVAL, NetworkWriteError, Request, Write, not_sent};

const CREATE: Request = Request::new(
    "network.CreateDHCPOptions",
    "v2/{}/dhcp_option",
    &Method::POST,
    &[StatusCode::CREATED],
);
const DELETE: Request = Request::new(
    "network.DeleteDHCPOptions",
    "v2/{}/dhcp_option/{}",
    &Method::DELETE,
    &[StatusCode::NO_CONTENT],
)
.repeatable();
const SET: Request = Request::new(
    "network.SetVPCDHCPOptions",
    "v2/{}/networks/{}/updateDhcpOption",
    &Method::PATCH,
    &[StatusCode::OK],
)
.repeatable();
const CLEAR: Request = Request::new(
    "network.ClearVPCDHCPOptions",
    "v2/{}/networks/{}/updateDhcpOption",
    &Method::PATCH,
    &[StatusCode::OK],
)
.repeatable();
const SYSTEM: &str = "dhcp-option-dns-";
const CREATE_ADVICE: &str = "the set may exist, run list-dhcp-options --name and match the name exactly before creating it again";

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a DHCP options set with the supplied IPv4 resolvers.
    pub async fn create_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: CreateDhcpOptionsInput,
    ) -> Result<CreateDhcpOptionsOutput, Failure<CreateDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            write.check(CREATE.id, &[("Name", &input.name)], &[])?;
            if input.dns_servers.is_empty() {
                return Err(super::wire::input(
                    CREATE.id,
                    "requires at least one DNS server",
                ));
            }
            if input
                .dns_servers
                .iter()
                .any(|value| value.parse::<Ipv4Addr>().is_err())
            {
                return Err(super::wire::input(
                    CREATE.id,
                    "requires DNSServers to be IPv4 addresses",
                ));
            }
            if input.name.starts_with(SYSTEM) {
                return Err(super::wire::input(
                    CREATE.id,
                    "requires Name without the reserved dhcp-option-dns- prefix",
                ));
            }
            let mut body = serde_json::Map::from_iter([
                ("name".into(), json!(input.name)),
                ("dnsServers".into(), json!(input.dns_servers)),
            ]);
            if let Some(mtu) = input.mtu {
                body.insert("mtu".into(), json!(mtu));
            }
            let reply = write
                .send(CREATE, &[], Some(body.into()))
                .await
                .map_err(|error| write.advise(error, CREATE_ADVICE))?;
            let dhcp_options = write.created(
                CREATE,
                reply,
                true,
                |row: &DhcpOptions| &row.uuid,
                false,
                CREATE_ADVICE,
            )?;
            Ok(CreateDhcpOptionsOutput { dhcp_options })
        })
    }

    /// Deletes an options set after verifying it has no VPC attachments.
    pub async fn delete_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: DeleteDhcpOptionsInput,
    ) -> Result<DeleteDhcpOptionsOutput, Failure<DeleteDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            let id = &input.dhcp_options_id;
            write.check(DELETE.id, &[], &[("DHCPOptionsID", id)])?;
            let current = write.dhcp(id).await.map_err(not_sent)?;
            if current.uuid != *id {
                return Err(not_sent(write.local(ErrorClass::Api, &format!("{}: get returned DHCP options set {:?} instead of the requested {id:?}; refusing to delete an unexpected set", DELETE.id, current.uuid))));
            }
            if !current.vpc_ids.is_empty() {
                return Err(not_sent(write.in_use(&format!("{}: DHCP options set {id} is attached to VPC(s) {}; detach it from every VPC first", DELETE.id, current.vpc_ids.join(", ")))));
            }
            let _ = write.send(DELETE, &[id], None).await?;
            Ok(DeleteDhcpOptionsOutput {})
        })
    }

    /// Attaches a DHCP options set and waits for the VPC to report it.
    pub async fn set_vpc_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: SetVpcDhcpOptionsInput,
    ) -> Result<SetVpcDhcpOptionsOutput, Failure<SetVpcDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            write.check(
                SET.id,
                &[],
                &[
                    ("VPCID", &input.vpc_id),
                    ("DHCPOptionsID", &input.dhcp_options_id),
                ],
            )?;
            write
                .change_dhcp(
                    SET,
                    &input.vpc_id,
                    Some(&input.dhcp_options_id),
                    |vpc, changed| SetVpcDhcpOptionsOutput { vpc, changed },
                )
                .await
        })
    }

    /// Detaches the VPC's DHCP options set and waits for the change to appear.
    pub async fn clear_vpc_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: ClearVpcDhcpOptionsInput,
    ) -> Result<ClearVpcDhcpOptionsOutput, Failure<ClearVpcDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut write = Write::new(
                self,
                ctx,
                crate::wire::product::evidence::Evidence::for_write(),
            )?;
            write.check(CLEAR.id, &[], &[("VPCID", &input.vpc_id)])?;
            write
                .change_dhcp(CLEAR, &input.vpc_id, None, |vpc, changed| {
                    ClearVpcDhcpOptionsOutput { vpc, changed }
                })
                .await
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Write<'_> {
    async fn change_dhcp<O: serde::Serialize>(
        &mut self,
        request: Request,
        id: &str,
        options: Option<&str>,
        output: impl FnOnce(Vpc, bool) -> O,
    ) -> Result<O, Failure<O>> {
        let op = request.id;
        let target_id = options.unwrap_or_default();
        let mut vpc = self.vpc(self.ctx, id).await.map_err(not_sent)?;
        if vpc.dhcp_option_id == target_id {
            return self.unchanged(op, output(vpc, false));
        }
        let current_name = if !vpc.dhcp_option_id.is_empty() && vpc.dhcp_option_name.is_empty() {
            let options = self.dhcp(&vpc.dhcp_option_id).await.map_err(not_sent)?;
            if options.uuid != vpc.dhcp_option_id {
                return Err(not_sent(self.invalid(op)));
            }
            options.name
        } else {
            vpc.dhcp_option_name.clone()
        };
        if vpc.dns_status != "DISABLED" || current_name.starts_with(SYSTEM) {
            let action = if options.is_some() {
                "setting another set"
            } else {
                "clearing it"
            };
            return Err(not_sent(self.guard_error(NetworkWriteError::DefaultResource,
                &format!("{op}: VPC {id} has Private DNS enabled or a system DHCP options set; {action} would cut it off from its private zones"))));
        }
        let body = if let Some(id) = options {
            let target = self.dhcp(id).await.map_err(not_sent)?;
            if target.uuid != id {
                return Err(not_sent(self.invalid(op)));
            }
            if target.name.starts_with(SYSTEM) {
                return Err(not_sent(self.guard_error(
                    NetworkWriteError::DefaultResource,
                    &format!("{op}: DHCP options set {id} is a system set"),
                )));
            }
            if target.status != "ACTIVE" {
                return Err(not_sent(self.guard_error(
                    NetworkWriteError::Busy,
                    &format!(
                        "{op}: DHCP options set {id} is {}, not ACTIVE",
                        target.status
                    ),
                )));
            }
            json!({"dhcpOptionId": id})
        } else {
            json!({})
        };
        let _ = self.send(request, &[id], Some(body)).await.map_err(|failure| {
            self.advise(failure, &format!("the change may already be in place; run get-vpc to see VPC {id}'s current DHCP options set"))
        })?;
        let want = options.map_or_else(
            || "an empty DHCP options set".to_owned(),
            |id| format!("DHCP options set {id}"),
        );
        let result = self.settle(BOUND, INTERVAL,
            &format!("{op}: VPC {id} did not show {want} within 1m0s; the PATCH was sent and is safe to repeat"),
            async |write, ctx| {
                vpc = write.vpc(ctx, id).await.map_err(|error| write.settled_error(&format!("{op}: VPC {id}"), error))?;
                if vpc.status == "ERROR" { return Err(write.status_error(op, &format!("VPC {id}"))); }
                Ok((vpc.dhcp_option_id == target_id).then_some(()))
            }).await;
        self.outcome(op, vpc, result, |vpc| output(vpc, true))
    }
}
