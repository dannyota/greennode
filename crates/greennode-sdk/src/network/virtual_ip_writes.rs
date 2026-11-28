use std::{net::Ipv4Addr, time::Duration};

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::wire::product::{
    evidence::Evidence,
    guard::{AcknowledgedId, guarded_poll},
};
use crate::{Error, ErrorClass, Failure, RequestContext};

use super::mutations::{
    Mutation, advise, check_active_wait, check_input, create_id, input_error, not_sent, path_id,
    retain, sent, settlement, tick_active_wait, typed_error,
};
use super::{Client, VirtualIpAddress};

const CREATE: Mutation = Mutation::new(
    "network.CreateVirtualIPAddress",
    "v2/{}/virtualIpAddress",
    Method::POST,
    &[StatusCode::CREATED],
);
const UPDATE: Mutation = Mutation::new(
    "network.UpdateVirtualIPAddress",
    "v2/{}/virtualIpAddress/{}",
    Method::PUT,
    &[StatusCode::OK],
)
.repeatable();
const DELETE: Mutation = Mutation::new(
    "network.DeleteVirtualIPAddress",
    "v2/{}/virtualIpAddress/{}",
    Method::DELETE,
    &[StatusCode::NO_CONTENT],
)
.repeatable();
const RECOVERY: &str = "list virtual ip addresses and match the name exactly, or the address if one was given, before creating it again";

/// Virtual IP mode that allows multiple active interfaces.
pub const VIRTUAL_IP_MODE_ACTIVE_ACTIVE: &str = "Active/Active";
/// Virtual IP mode that permits one active interface.
pub const VIRTUAL_IP_MODE_ACTIVE_PASSIVE: &str = "Active/Passive";

/// Defines a new private virtual IP address.
#[derive(Clone)]
pub struct CreateVirtualIpAddressInput {
    /// Target subnet identifier.
    pub subnet_id: String,
    /// Display name.
    pub name: String,
    /// Virtual IP mode.
    pub mode: String,
    /// Requested IPv4 address. An empty value lets the service choose.
    pub ip_address: String,
    /// Display description.
    pub description: String,
}

impl CreateVirtualIpAddressInput {
    /// Sets the subnet, name and mode required to create the virtual IP.
    pub fn new(
        subnet_id: impl Into<String>,
        name: impl Into<String>,
        mode: impl Into<String>,
    ) -> Self {
        Self {
            subnet_id: subnet_id.into(),
            name: name.into(),
            mode: mode.into(),
            ip_address: String::new(),
            description: String::new(),
        }
    }
}

/// The newly created virtual IP address.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateVirtualIpAddressOutput {
    /// Last observed virtual IP address.
    #[serde(rename = "VirtualIPAddress")]
    pub virtual_ip_address: VirtualIpAddress,
}

/// Changes selected fields of a virtual IP address.
#[derive(Clone)]
pub struct UpdateVirtualIpAddressInput {
    /// Virtual IP identifier.
    pub virtual_ip_address_id: String,
    /// Replacement display name.
    pub name: Option<String>,
    /// Replacement description.
    pub description: Option<String>,
    /// Replacement mode.
    pub mode: Option<String>,
}

impl UpdateVirtualIpAddressInput {
    /// Selects the virtual IP to update.
    pub fn new(virtual_ip_address_id: impl Into<String>) -> Self {
        Self {
            virtual_ip_address_id: virtual_ip_address_id.into(),
            name: None,
            description: None,
            mode: None,
        }
    }
}

/// The virtual IP address after the update.
#[derive(Clone, PartialEq, Serialize)]
pub struct UpdateVirtualIpAddressOutput {
    /// Confirmed virtual IP, or the write response if confirmation failed.
    #[serde(rename = "VirtualIPAddress")]
    pub virtual_ip_address: VirtualIpAddress,
}

super::route_inventory::required_id_input!(
    /// Selects an unused private virtual IP address for deletion.
    DeleteVirtualIpAddressInput,
    virtual_ip_address_id
);

/// Confirms that the service accepted the deletion.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DeleteVirtualIpAddressOutput {}

/// An attached resource prevented deletion.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ResourceInUse;

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Creates a virtual IP and waits for ACTIVE if the create response is still pending.
    pub async fn create_virtual_ip_address(
        &self,
        ctx: &RequestContext,
        input: CreateVirtualIpAddressInput,
    ) -> Result<CreateVirtualIpAddressOutput, Failure<CreateVirtualIpAddressOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self
                .request_evidence_from(ctx, Evidence::for_write())
                .map_err(not_sent)?;
            let body = json!({"subnetId": input.subnet_id, "name": input.name, "mode": input.mode, "ipAddress": input.ip_address, "description": input.description});
            check_input(
                CREATE.id,
                &evidence,
                &[
                    ("SubnetID", &input.subnet_id),
                    ("Name", &input.name),
                    ("Mode", &input.mode),
                ],
                &[],
                Some(&body),
            )
            .map_err(not_sent)?;
            if !input.ip_address.is_empty() && input.ip_address.parse::<Ipv4Addr>().is_err() {
                return Err(not_sent(input_error(
                    CREATE.id,
                    ": IPAddress must be an IPv4 address such as 10.20.0.10",
                )));
            }
            let raw = self
                .mutate_vserver(ctx, &CREATE, &[], Some(&body), &mut evidence)
                .await
                .map_err(|failure| advise(failure, CREATE.id, RECOVERY, &evidence))?;
            let row: VirtualIpAddress = CREATE
                .decode(&raw, &evidence)
                .map_err(|error| advise(sent(error), CREATE.id, RECOVERY, &evidence))?;
            let pending = row.status != "ACTIVE";
            create_id(&CREATE, &row.uuid, pending, "create response had no id; the virtual IP may exist, list virtual ip addresses and match the name exactly, or the address if one was given, before creating it again", &evidence).map_err(sent)?;
            let acknowledged =
                super::mutations::bind_acknowledgement(&CREATE, &raw, &row.uuid, &mut evidence)
                    .map_err(sent)?;
            let mut row = retain(row, &evidence).ok_or_else(|| sent(CREATE.invalid(&evidence)))?;
            let result = if pending {
                self.wait_virtual_ip(ctx, &acknowledged, &mut row, &mut evidence)
                    .await
            } else {
                Ok(())
            };
            let id = acknowledged
                .recheck(&evidence)
                .map_err(|error| sent(Error::new(ErrorClass::NotSettled).with_cause(*error)))?;
            let output = retain(row, &evidence)
                .map(|virtual_ip_address| CreateVirtualIpAddressOutput { virtual_ip_address });
            match result {
                Ok(()) => output.ok_or_else(|| sent(CREATE.invalid(&evidence))),
                Err(error) => Err(settlement(
                    CREATE.id,
                    "virtual IP",
                    id,
                    error,
                    output,
                    &evidence,
                )),
            }
        })
    }

    async fn wait_virtual_ip(
        &self,
        ctx: &RequestContext,
        acknowledged: &AcknowledgedId,
        latest: &mut VirtualIpAddress,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(60));
        guarded_poll!(evidence, {
            check_active_wait(&wait, ctx, clock)?;
            let id = acknowledged.recheck(evidence).map_err(|error| *error)?;
            let result = self.virtual_ip_detail(&wait, id, true, evidence).await;
            acknowledged.recheck(evidence).map_err(|error| *error)?;
            evidence.recheck_read_proofs().map_err(|error| *error)?;
            match result {
                Ok(row) => {
                    *latest = row;
                    match latest.status.as_str() {
                        "ACTIVE" => return check_active_wait(&wait, ctx, clock),
                        "ERROR" => return Err(Error::new(ErrorClass::Failed)),
                        _ => {}
                    }
                }
                Err(error)
                    if crate::confirms_absence(
                        &error,
                        "network.GetVirtualIPAddress",
                        "GET",
                        "v2/{}/virtualIpAddress/{}",
                    ) => {}
                Err(error) => return Err(error),
            }
            tick_active_wait(&wait, ctx, clock).await?;
        })
    }

    /// Sends an update for the virtual IP and reads its detail to confirm the result.
    pub async fn update_virtual_ip_address(
        &self,
        ctx: &RequestContext,
        input: UpdateVirtualIpAddressInput,
    ) -> Result<UpdateVirtualIpAddressOutput, Failure<UpdateVirtualIpAddressOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.virtual_ip_address_id.as_str();
            path_id(UPDATE.id, "VirtualIPAddressID", id).map_err(not_sent)?;
            if input.name.is_none() && input.description.is_none() && input.mode.is_none() {
                return Err(not_sent(input_error(
                    UPDATE.id,
                    " requires at least one field to change",
                )));
            }
            let mut evidence = self
                .request_evidence_from(ctx, Evidence::for_write())
                .map_err(not_sent)?;
            check_input(
                UPDATE.id,
                &evidence,
                &[],
                &[id],
                Some(&json!([input.name, input.description, input.mode])),
            )
            .map_err(not_sent)?;
            let row = self
                .virtual_ip_detail(ctx, id, true, &mut evidence)
                .await
                .map_err(not_sent)?;
            let body = json!({"name": input.name.unwrap_or(row.name), "description": input.description.unwrap_or(row.description), "mode": input.mode.unwrap_or(row.mode)});
            let raw = self
                .mutate_vserver(ctx, &UPDATE, &[id], Some(&body), &mut evidence)
                .await?;
            let fallback: VirtualIpAddress = UPDATE.decode(&raw, &evidence).map_err(sent)?;
            if !fallback.uuid.is_empty() && fallback.uuid != id {
                return Err(sent(UPDATE.invalid(&evidence)));
            }
            let fallback =
                retain(fallback, &evidence).ok_or_else(|| sent(UPDATE.invalid(&evidence)))?;
            let output = |virtual_ip_address| UpdateVirtualIpAddressOutput { virtual_ip_address };
            match self.virtual_ip_detail(ctx, id, true, &mut evidence).await {
                Ok(row) => Ok(output(row)),
                Err(error) => Err(settlement(
                    UPDATE.id,
                    "virtual IP",
                    id,
                    error,
                    retain(fallback, &evidence).map(output),
                    &evidence,
                )),
            }
        })
    }

    /// Deletes a private virtual IP after confirming it has no address pairs.
    pub async fn delete_virtual_ip_address(
        &self,
        ctx: &RequestContext,
        input: DeleteVirtualIpAddressInput,
    ) -> Result<DeleteVirtualIpAddressOutput, Failure<DeleteVirtualIpAddressOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.virtual_ip_address_id.as_str();
            path_id(DELETE.id, "VirtualIPAddressID", id).map_err(not_sent)?;
            let mut evidence = self
                .request_evidence_from(ctx, Evidence::for_write())
                .map_err(not_sent)?;
            check_input(DELETE.id, &evidence, &[], &[id], None).map_err(not_sent)?;
            let row = self
                .virtual_ip_detail(ctx, id, true, &mut evidence)
                .await
                .map_err(not_sent)?;
            if row.kind != "private" {
                return Err(not_sent(input_error(
                    DELETE.id,
                    ": virtual IP is not a private virtual IP; it must be deleted through the public virtual IP call instead",
                )));
            }
            let count = if row.address_pair_ips.is_empty() {
                self.guard_vip_pairs(ctx, id, &mut evidence)
                    .await
                    .map_err(not_sent)?
                    .len()
            } else {
                row.address_pair_ips.len()
            };
            if count != 0 {
                let message = format!(
                    "network: resource in use: {}: virtual IP {id} has {count} address pair(s) attached",
                    DELETE.id
                );
                return Err(not_sent(
                    typed_error(ErrorClass::Api, "ResourceInUse", &message, &evidence)
                        .with_detail(ResourceInUse),
                ));
            }
            let _ = self
                .mutate_vserver(ctx, &DELETE, &[id], None, &mut evidence)
                .await?;
            Ok(DeleteVirtualIpAddressOutput {})
        })
    }
}

crate::wire::product::output::public_fields!(CreateVirtualIpAddressOutput { virtual_ip_address });
crate::wire::product::output::public_fields!(UpdateVirtualIpAddressOutput { virtual_ip_address });
crate::wire::product::output::public_fields!(DeleteVirtualIpAddressOutput {});

crate::error::debug_record!(CreateVirtualIpAddressInput {
    subnet_id,
    name,
    mode,
    ip_address,
    description
});
crate::error::debug_record!(UpdateVirtualIpAddressInput {
    virtual_ip_address_id,
    name,
    description,
    mode
});
crate::error::debug_record!(ResourceInUse unit);
