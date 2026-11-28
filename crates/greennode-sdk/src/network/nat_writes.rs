use std::{collections::HashSet, time::Duration};

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, value::RawValue};

use crate::config::session::AuthKind;
use crate::error::SafeText;
use crate::model::wire::go_json;
use crate::wire::product::{
    evidence::Evidence,
    guard::guarded_poll,
    options::{Options, PathIds},
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::mutations::{
    Destination, Mutation, api, check_context, check_input, not_sent, path_id, safe_output, screen,
    tick, typed_error,
};
use super::nat_catalog::{NatScope, decode_guard_instances, numeric_code, params};
use super::wire::{Fields, Read, Style};
use super::{Client, NatInstance};

const DELETE: Mutation = Mutation::new(
    "network.DeleteNATInstance",
    "vnetwork/v1/{}/{}/nats/{}",
    Method::DELETE,
    &[StatusCode::OK],
)
.protected(false, "NAT response withheld");
const RECOVERY: &str = "Run network list-nat-instances in the same scope and billing list-resources, matching the known NAT ID. If no NAT appears, inspect payment history without ordering again. Renewal may remain enabled; use the billing console for a deliberate disable and confirm by a billing read. Do not repeat create while purchase or renewal is unresolved. Deletion requires a separate deliberate action with VPC consent.";

/// Selects a NAT instance and its owning VPC for deletion.
#[derive(Clone)]
pub struct DeleteNatInstanceInput {
    /// Regional vNetwork zone.
    pub zone_id: String,
    /// Owning VPC identifier.
    pub vpc_id: String,
    /// NAT instance identifier.
    pub nat_id: String,
    /// Return after acceptance without confirming absence.
    pub no_wait: bool,
}

impl DeleteNatInstanceInput {
    /// Sets the zone, VPC and NAT that the deletion must match.
    pub fn new(
        zone_id: impl Into<String>,
        vpc_id: impl Into<String>,
        nat_id: impl Into<String>,
    ) -> Self {
        Self {
            zone_id: zone_id.into(),
            vpc_id: vpc_id.into(),
            nat_id: nat_id.into(),
            no_wait: false,
        }
    }
}

/// A successful NAT deletion.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DeleteNatInstanceOutput {}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Deletes one NAT after checking VPC ownership and optionally confirms its absence.
    pub async fn delete_nat_instance(
        &self,
        ctx: &RequestContext,
        input: DeleteNatInstanceInput,
    ) -> Result<DeleteNatInstanceOutput, Failure<DeleteNatInstanceOutput>> {
        crate::wire::product::exit::finish!(async {
            let op = DELETE.id;
            let mut evidence = self
                .request_evidence_from(ctx, Evidence::for_write())
                .map_err(not_sent)?;
            self.nat_write_auth(op, &evidence).map_err(not_sent)?;
            for (field, id) in [
                ("ZoneID", &input.zone_id),
                ("VPCID", &input.vpc_id),
                ("NATID", &input.nat_id),
            ] {
                path_id(op, field, id).map_err(not_sent)?;
            }
            check_input(
                op,
                &evidence,
                &[],
                &[&input.zone_id, &input.vpc_id, &input.nat_id],
                None,
            )
            .map_err(not_sent)?;
            let read = Read::new(op, "vnetwork/v1/{}/{}/nats", Style::NatCatalog);
            let scope = self
                .nat_scope(ctx, read, input.zone_id, &mut evidence, false)
                .await
                .map_err(not_sent)?;
            self.check_nat_vpc(ctx, &scope, &input.vpc_id, op, &mut evidence)
                .await
                .map_err(|error| not_sent(nat_error(op, error, &evidence)))?;
            let rows = self
                .nat_write_inventory(ctx, &scope, read, &mut evidence)
                .await
                .map_err(not_sent)?;
            let Some(row) = rows.iter().find(|row| row.uuid == input.nat_id) else {
                return Err(not_sent(typed_error(
                    ErrorClass::NotFound,
                    "NotFound",
                    &format!("vngcloud: resource not found: {op}"),
                    &evidence,
                )));
            };
            if row.vpc.uuid != input.vpc_id {
                return Err(not_sent(super::mutations::input_error(
                    op,
                    ": NAT does not belong to the named VPC",
                )));
            }
            let path = [&*scope.zone, &*scope.project, &*input.nat_id];
            let target = Destination {
                base: &scope.base,
                path: &path,
                options: Options::DEFAULT,
            };
            let sent = self
                .mutate::<DeleteNatInstanceOutput>(
                    ctx,
                    &DELETE,
                    target,
                    Some(&json!({})),
                    &mut evidence,
                )
                .await;
            let send_error = match sent {
                Ok(raw) => raw
                    .decode_projection(|body| nat_ack(op, body, &evidence).map_err(Box::new))
                    .err()
                    .map(|error| *error),
                Err(failure) if failure.write_dispatch() == WriteDispatch::NotSent => {
                    return Err(failure);
                }
                Err(failure) => Some(nat_error(op, failure.into_parts().0, &evidence)),
            };
            let output = DeleteNatInstanceOutput {};
            let interrupted = send_error.as_ref().is_some_and(|error| {
                error
                    .http_status()
                    .as_ref()
                    .is_some_and(|status| !status.is_redirection())
                    && error
                        .api_error()
                        .and_then(ApiError::observed_response_status)
                        .is_none()
            });
            if input.no_wait {
                return match send_error {
                    None => Ok(output),
                    Some(error) => Err(nat_failure(op, Some(error), None, output, &evidence)),
                };
            }
            let clock = self.session.clock();
            let wait = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(600));
            let result = async {
                guarded_poll!(evidence, {
                    check_context(&wait, clock)?;
                    let rows = self
                        .nat_write_inventory(&wait, &scope, read, &mut evidence)
                        .await?;
                    check_context(&wait, clock)?;
                    let present = rows.iter().any(|row| row.uuid == input.nat_id);
                    if interrupted || send_error.as_ref().is_some_and(definite_refusal) {
                        return Ok(present);
                    }
                    if !present {
                        return Ok(false);
                    }
                    tick(&wait, clock, Duration::from_secs(5)).await?;
                })
            }
            .await;
            match result {
                Ok(_) if interrupted => Err(nat_failure(op, send_error, None, output, &evidence)),
                Ok(true) => match send_error {
                    Some(error) => Err(Failure::new(
                        error,
                        Some(output),
                        WriteDispatch::MayHaveBeenSent,
                    )),
                    None => Err(nat_failure(op, None, None, output, &evidence)),
                },
                Ok(false) if !interrupted && !send_error.as_ref().is_some_and(definite_refusal) => {
                    Ok(output)
                }
                Ok(false) => Err(nat_failure(op, send_error, None, output, &evidence)),
                Err(error) => Err(nat_failure(op, send_error, Some(error), output, &evidence)),
            }
        })
    }

    pub(super) fn nat_write_auth(
        &self,
        op: &'static str,
        evidence: &Evidence,
    ) -> Result<(), Error> {
        let trusted_fixture = {
            #[cfg(any(test, feature = "test-support"))]
            {
                self.session
                    .is_write_fixture(crate::testing::Product::Vnetwork)
            }
            #[cfg(not(any(test, feature = "test-support")))]
            {
                false
            }
        };
        if !matches!(self.region.as_str(), "han-1" | "hcm-3")
            || !(self.session.auth_kind() == AuthKind::IamUser || trusted_fixture)
        {
            return Err(typed_error(
                ErrorClass::InvalidConfig,
                "InvalidConfig",
                &format!(
                    "vngcloud: invalid config: {op} requires SDK IAM-user login in hcm-3 or han-1"
                ),
                evidence,
            ));
        }
        Ok(())
    }

    pub(super) async fn nat_write_inventory(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        read: Read,
        evidence: &mut Evidence,
    ) -> Result<Vec<NatInstance>, Error> {
        let mut rows = Vec::new();
        let mut ids = HashSet::new();
        let mut totals = None;
        let mut retained = 0_usize;
        for page in 1..=50 {
            let query = params(page, 1000);
            let raw = self
                .nat_catalog_request(ctx, read, scope, &[("params", &query)], evidence)
                .await?;
            retained = retained.saturating_add(raw.len());
            let invalid_page = || invalid(read.id, evidence);
            if retained > 4 << 20 {
                return Err(invalid_page());
            }
            let text = go_json(&raw);
            check_inventory_members(&text).map_err(|()| invalid_page())?;
            let output = decode_guard_instances(&text, evidence).map_err(|_| invalid_page())?;
            let output = super::mutations::guard_operands(read.id, output, evidence)?;
            let invalid = || invalid(read.id, evidence);
            let page_info = (output.page_size, output.total_page, output.total_item);
            if page != output.page
                || !(0..=50).contains(&output.total_page)
                || totals.is_some_and(|old| old != page_info)
                || i64::try_from(output.items.len()).map_or(true, |count| count > output.page_size)
                || (output.total_item > 0 && (output.total_page == 0 || output.items.is_empty()))
                || (output.total_item == 0 && (output.total_page != 0 || !output.items.is_empty()))
            {
                return Err(invalid());
            }
            totals = Some(page_info);
            for row in output.items {
                if !PathIds::Strict.allows(&row.uuid)
                    || !PathIds::Strict.allows(&row.vpc.uuid)
                    || !PathIds::Strict.allows(&row.nat_package.uuid)
                    || row.nat_name.is_empty()
                    || row.status.is_empty()
                    || row.project_uuid != scope.project
                    || row.zone_uuid.is_empty()
                    || row.vpc.region_id != scope.zone
                    || !ids.insert(row.uuid.clone())
                {
                    return Err(invalid());
                }
                rows.push(row);
            }
            let count = i64::try_from(rows.len()).map_err(|_| invalid())?;
            if count > output.total_item {
                return Err(invalid());
            }
            if page >= output.total_page {
                if count != output.total_item || !safe_output(&rows, evidence) {
                    return Err(invalid());
                }
                return Ok(rows);
            }
        }
        Err(invalid(read.id, evidence))
    }
}

fn check_inventory_members(text: &str) -> Result<(), ()> {
    let fields = Fields::decode(text)?;
    let Some(data) = fields.raw("data")? else {
        return Ok(());
    };
    let rows: Vec<Fields<'_>> = serde_json::from_str(data.get()).map_err(|_| ())?;
    for row in rows {
        for name in [
            "uuid",
            "natName",
            "status",
            "projectUuid",
            "zoneUuid",
            "vpc",
            "natPackage",
        ] {
            if row.raw(name)?.is_none_or(|raw| raw.get() == "null") {
                return Err(());
            }
        }
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn nat_ack(op: &'static str, raw: &[u8], evidence: &Evidence) -> Result<(), Error> {
    let text = go_json(raw);
    let fail = |()| invalid(op, evidence);
    let fields = Fields::decode(&text).map_err(fail)?;
    fields.check_ack_paging().map_err(fail)?;
    let code: &RawValue = fields.required("code").map_err(fail)?;
    if !fields.required::<bool>("success").map_err(fail)? || !numeric_code(code.get(), 0) {
        return Err(fail(()));
    }
    Ok(())
}

pub(super) fn invalid(op: &'static str, evidence: &Evidence) -> Error {
    api(
        op,
        Some(200),
        "InvalidResponse",
        "NAT response had invalid structure",
        evidence,
    )
}

pub(super) fn nat_error(op: &'static str, error: Error, evidence: &Evidence) -> Error {
    let Some(detail) = error.api_error() else {
        return screen(error, evidence);
    };
    let code = SafeText::new(detail.code(), &[]);
    let message = SafeText::new(&detail.message().replace(super::nat_quote::OP, op), &[]);
    screen(
        error
            .with_api_operation(op)
            .with_api_diagnostic(code, message),
        evidence,
    )
}

pub(super) fn definite_refusal(error: &Error) -> bool {
    error
        .api_error()
        .and_then(ApiError::observed_response_status)
        .is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 429))
}

pub(super) fn nat_failure<T: Serialize>(
    op: &'static str,
    send: Option<Error>,
    wait: Option<Error>,
    output: T,
    evidence: &Evidence,
) -> Failure<T> {
    let message = format!("{op}: write outcome is unconfirmed; {RECOVERY}");
    let mut error = typed_error(ErrorClass::NotSettled, "NotSettled", &message, evidence);
    for cause in send.into_iter().chain(wait) {
        error = error.with_cause(cause);
    }
    let partial = safe_output(&output, evidence).then_some(output);
    Failure::new(
        screen(error, evidence),
        partial,
        WriteDispatch::MayHaveBeenSent,
    )
}

crate::wire::product::output::public_fields!(DeleteNatInstanceOutput {});

crate::error::debug_record!(DeleteNatInstanceInput {
    zone_id,
    vpc_id,
    nat_id,
    no_wait
});
