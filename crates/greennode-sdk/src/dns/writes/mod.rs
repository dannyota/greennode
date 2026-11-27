mod inputs;
mod wire;
mod workflow;

pub use inputs::{
    CreateHostedZoneInput, CreateHostedZoneOutput, CreateRecordInput, CreateRecordOutput,
    DeleteHostedZoneInput, DeleteHostedZoneOutput, DeleteRecordInput, DeleteRecordOutput,
    UpdateHostedZoneInput, UpdateHostedZoneOutput, UpdateRecordInput, UpdateRecordOutput,
};
pub use workflow::ErrorDetail;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;

use super::{Client, Field, HostedZone, Record, RecordValue, api_error, validate_ids};
use crate::error::SafeText;
use crate::{ApiError, Error, Failure, RequestContext, WriteDispatch};
use wire::{Acknowledgement, Detail, RecordValueBody};
use workflow::{
    ACTIVE, RECORD, RECORDS, Workflow, ZONE, ZONES, discard_partial, encode, failure_reason,
    invalid, not_sent,
};

const CREATE_ZONE: &str = "dns.CreateHostedZone";
const UPDATE_ZONE: &str = "dns.UpdateHostedZone";
const DELETE_ZONE: &str = "dns.DeleteHostedZone";
const CREATE_RECORD: &str = "dns.CreateRecord";
const UPDATE_RECORD: &str = "dns.UpdateRecord";
const DELETE_RECORD: &str = "dns.DeleteRecord";

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a private zone. Unless `no_wait` is set, waits for it to become ACTIVE.
    pub async fn create_hosted_zone(
        &self,
        ctx: &RequestContext,
        input: CreateHostedZoneInput,
    ) -> Result<CreateHostedZoneOutput, Failure<CreateHostedZoneOutput>> {
        crate::wire::product::exit::finish!(async {
            require(
                CREATE_ZONE,
                &[("DomainName", !input.domain_name.is_empty())],
            )?;
            let body = encode(&CreateZoneBody {
                domain_name: &input.domain_name,
                assoc_vpc_ids: &input.vpc_ids,
                zone_type: "PRIVATE",
                description: &input.description,
            })?;
            let mut flow = Workflow::start(self, ctx, CREATE_ZONE).await?;
            let (status, response) = flow
                .send::<Detail<Option<HostedZone>>>(Method::POST, ZONES, &[], body)
                .await
                .map_err(create_failure(CREATE_ZONE))?;
            let zone = response
                .data
                .filter(|zone| !zone.id.is_empty())
                .ok_or_else(|| missing_id(CREATE_ZONE, status))?;
            if input.no_wait {
                return Ok(CreateHostedZoneOutput {
                    hosted_zone: flow.clean(zone),
                });
            }
            let id = zone.id.clone();
            flow.settle_zone(
                &id,
                zone,
                false,
                |zone| zone.status == ACTIVE,
                |hosted_zone| CreateHostedZoneOutput { hosted_zone },
            )
            .await
        })
    }

    /// Reads a complete snapshot and sends one replacement. An uncertain write retains safe partial zone data.
    pub async fn update_hosted_zone(
        &self,
        ctx: &RequestContext,
        input: UpdateHostedZoneInput,
    ) -> Result<UpdateHostedZoneOutput, Failure<UpdateHostedZoneOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.hosted_zone_id.as_str();
            validate_ids(UPDATE_ZONE, &[("HostedZoneID", id)]).map_err(|error| not_sent(*error))?;
            require_change(
                UPDATE_ZONE,
                input.description.is_some() || input.vpc_ids.is_some(),
            )?;
            let mut flow = Workflow::start(self, ctx, UPDATE_ZONE).await?;
            let current = flow.ready(id).await?;
            if (input.description.is_none()
                && flow
                    .evidence
                    .credentials()
                    .reflects(current.description.as_bytes()))
                || (input.vpc_ids.is_none()
                    && current
                        .associated_vpc_ids
                        .iter()
                        .any(|vpc| flow.evidence.credentials().reflects(vpc.as_bytes())))
            {
                return Err(not_sent(api_error(
                    "",
                    "",
                    "dns.UpdateHostedZone: incomplete hosted zone replacement snapshot",
                )));
            }
            let description = input.description.unwrap_or(current.description);
            let assoc_vpc_ids = input.vpc_ids.unwrap_or(current.associated_vpc_ids);
            let body = encode(&UpdateZoneBody {
                description: &description,
                assoc_vpc_ids: &assoc_vpc_ids,
            })?;
            let fallback = HostedZone {
                id: id.into(),
                description,
                associated_vpc_ids: assoc_vpc_ids,
                ..HostedZone::default()
            };
            if let Err(err) = flow
                .send::<Acknowledgement>(Method::PUT, ZONE, &[id], body)
                .await
            {
                let (error, _, dispatch) = err.into_parts();
                if dispatch == WriteDispatch::NotSent || is_rejected(&error) {
                    return Err(Failure::new(error, None, dispatch));
                }
                let error = flow
                    .unconfirmed(&format!(
                        "replacement outcome is uncertain; read the zone before another write: {}",
                        failure_reason(&error)
                    ))
                    .with_cause(error);
                return Err(flow.after_write(error, || UpdateHostedZoneOutput {
                    hosted_zone: flow.clean(fallback),
                }));
            }
            let description = fallback.description.clone();
            let vpc_ids = fallback.associated_vpc_ids.clone();
            flow.settle_zone(
                id,
                fallback,
                input.no_wait,
                |zone| {
                    zone.status == ACTIVE
                        && zone.description == description
                        && same_vpcs(&zone.associated_vpc_ids, &vpc_ids)
                },
                |hosted_zone| UpdateHostedZoneOutput { hosted_zone },
            )
            .await
        })
    }

    /// Deletes one hosted zone. Unless `no_wait` is set, waits until the zone is no longer found.
    pub async fn delete_hosted_zone(
        &self,
        ctx: &RequestContext,
        input: DeleteHostedZoneInput,
    ) -> Result<DeleteHostedZoneOutput, Failure<DeleteHostedZoneOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.hosted_zone_id.as_str();
            validate_ids(DELETE_ZONE, &[("HostedZoneID", id)]).map_err(|error| not_sent(*error))?;
            let mut flow = Workflow::start(self, ctx, DELETE_ZONE).await?;
            flow.ready(id).await?;
            flow.send::<Acknowledgement>(Method::DELETE, ZONE, &[id], Bytes::new())
                .await
                .map_err(discard_partial)?;
            let output = DeleteHostedZoneOutput {};
            if input.no_wait {
                Ok(output)
            } else {
                flow.wait_gone(id, None, output).await
            }
        })
    }

    /// Creates one record in a ready zone. Unless `no_wait` is set, waits for both to become ACTIVE.
    pub async fn create_record(
        &self,
        ctx: &RequestContext,
        input: CreateRecordInput,
    ) -> Result<CreateRecordOutput, Failure<CreateRecordOutput>> {
        crate::wire::product::exit::finish!(async {
            let zone_id = input.hosted_zone_id.as_str();
            require(
                CREATE_RECORD,
                &[
                    ("HostedZoneID", !zone_id.is_empty()),
                    ("Type", !input.record_type.is_empty()),
                ],
            )?;
            validate_ids(CREATE_RECORD, &[("HostedZoneID", zone_id)])
                .map_err(|error| not_sent(*error))?;
            let values = record_values(&input.values)?;
            let body = encode(&RecordBody {
                sub_domain: Some(&input.sub_domain),
                ttl: Some(if input.ttl == 0 { 300 } else { input.ttl }),
                record_type: Some(&input.record_type),
                routing_policy: Some(if input.routing_policy.is_empty() {
                    "simple-routing"
                } else {
                    &input.routing_policy
                }),
                value: Some(values),
                enable_sticky_session: input.sticky_session,
            })?;
            let mut flow = Workflow::start(self, ctx, CREATE_RECORD).await?;
            flow.ready(zone_id).await?;
            let (status, response) = flow
                .send::<Detail<Option<Record>>>(Method::POST, RECORDS, &[zone_id], body)
                .await
                .map_err(create_failure(CREATE_RECORD))?;
            let record = response
                .data
                .filter(|record| !record.id.is_empty())
                .ok_or_else(|| missing_id(CREATE_RECORD, status))?;
            if input.no_wait {
                return Ok(CreateRecordOutput {
                    record: flow.clean(record),
                });
            }
            let id = record.id.clone();
            flow.settle_record(
                (zone_id, &id),
                record,
                false,
                |record, _| record.status == ACTIVE,
                |record| CreateRecordOutput { record },
            )
            .await
        })
    }

    /// Updates supplied fields once a zone is ready. With `no_wait`, reads the record once after the write.
    pub async fn update_record(
        &self,
        ctx: &RequestContext,
        input: UpdateRecordInput,
    ) -> Result<UpdateRecordOutput, Failure<UpdateRecordOutput>> {
        crate::wire::product::exit::finish!(async {
            let path = (&*input.hosted_zone_id, &*input.record_id);
            validate_ids(
                UPDATE_RECORD,
                &[("HostedZoneID", path.0), ("RecordID", path.1)],
            )
            .map_err(|error| not_sent(*error))?;
            require_change(
                UPDATE_RECORD,
                input.sub_domain.is_some()
                    || input.record_type.is_some()
                    || input.ttl.is_some()
                    || input.routing_policy.is_some()
                    || input.values.is_some()
                    || input.sticky_session.is_some(),
            )?;
            let body = encode(&RecordBody {
                sub_domain: input.sub_domain.as_deref(),
                ttl: input.ttl,
                record_type: input.record_type.as_deref(),
                routing_policy: input.routing_policy.as_deref(),
                value: input.values.as_deref().map(record_values).transpose()?,
                enable_sticky_session: input.sticky_session,
            })?;
            let mut flow = Workflow::start(self, ctx, UPDATE_RECORD).await?;
            flow.ready(path.0).await?;
            flow.send::<Acknowledgement>(Method::PUT, RECORD, &[path.0, path.1], body)
                .await
                .map_err(discard_partial)?;
            let fallback = Record {
                id: path.1.into(),
                hosted_zone_id: path.0.into(),
                sub_domain: input.sub_domain.clone().unwrap_or_default(),
                record_type: input.record_type.clone().unwrap_or_default(),
                ttl: Field::Value(input.ttl.unwrap_or_default()),
                routing_policy: input.routing_policy.clone().unwrap_or_default(),
                value: input.values.clone().unwrap_or_default(),
                enable_sticky_session: input.sticky_session.map(Field::Value),
                ..Record::default()
            };
            let mut polled = false;
            flow.settle_record(
                path,
                fallback,
                input.no_wait,
                |record, zone| {
                    let first = !std::mem::replace(&mut polled, true);
                    !first && record.status == ACTIVE && matches_update(record, zone, &input)
                },
                |record| UpdateRecordOutput { record },
            )
            .await
        })
    }

    /// Deletes one record in a ready zone. Unless `no_wait` is set, waits until the record is no longer found.
    pub async fn delete_record(
        &self,
        ctx: &RequestContext,
        input: DeleteRecordInput,
    ) -> Result<DeleteRecordOutput, Failure<DeleteRecordOutput>> {
        crate::wire::product::exit::finish!(async {
            let path = (&*input.hosted_zone_id, &*input.record_id);
            validate_ids(
                DELETE_RECORD,
                &[("HostedZoneID", path.0), ("RecordID", path.1)],
            )
            .map_err(|error| not_sent(*error))?;
            let mut flow = Workflow::start(self, ctx, DELETE_RECORD).await?;
            flow.ready(path.0).await?;
            flow.send::<Acknowledgement>(Method::DELETE, RECORD, &[path.0, path.1], Bytes::new())
                .await
                .map_err(discard_partial)?;
            let output = DeleteRecordOutput {};
            if input.no_wait {
                Ok(output)
            } else {
                flow.wait_gone(path.0, Some(path.1), output).await
            }
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateZoneBody<'a> {
    domain_name: &'a str,
    assoc_vpc_ids: &'a [String],
    #[serde(rename = "type")]
    zone_type: &'static str,
    description: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateZoneBody<'a> {
    assoc_vpc_ids: &'a [String],
    description: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_domain: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<i64>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    record_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_policy: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Vec<RecordValueBody<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_sticky_session: Option<bool>,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn record_values<T>(values: &[RecordValue]) -> Result<Vec<RecordValueBody<'_>>, Failure<T>> {
    values
        .iter()
        .map(RecordValueBody::try_from)
        .collect::<Result<_, ()>>()
        .map_err(|()| {
            not_sent(invalid(
                "vngcloud: invalid input: Values contains a redacted weight",
            ))
        })
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn require<T>(operation: &str, fields: &[(&str, bool)]) -> Result<(), Failure<T>> {
    match fields.iter().find(|(_, present)| !present) {
        Some((field, _)) => Err(not_sent(invalid(&format!(
            "vngcloud: invalid input: {operation} requires {field}"
        )))),
        None => Ok(()),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn require_change<T>(operation: &str, changed: bool) -> Result<(), Failure<T>> {
    if changed {
        Ok(())
    } else {
        Err(not_sent(invalid(&format!(
            "vngcloud: invalid input: {operation} requires at least one field to change"
        ))))
    }
}

fn missing_id<T>(id: &'static str, status: StatusCode) -> Failure<T> {
    let error = ApiError::without_fallback(
        id,
        Some(status.as_u16()),
        SafeText::new("", &[]),
        SafeText::new("create response had no id", &[]),
        false,
    );
    Failure::new(Error::from_api(error), None, WriteDispatch::MayHaveBeenSent)
}

fn create_failure<T, O>(operation: &'static str) -> impl FnOnce(Failure<T>) -> Failure<O> {
    move |failure| {
        let (error, _, dispatch) = failure.into_parts();
        let error = if dispatch == WriteDispatch::MayHaveBeenSent
            && !observed_status(&error).is_some_and(|status| status.is_client_error())
        {
            let api = error.api_error();
            let code = api.map_or("", ApiError::code);
            let message = format!(
                "{operation}: create may have already reached the server; list before creating it again: {}",
                failure_reason(&error)
            );
            Error::from_api(ApiError::without_fallback(
                api.map_or("", ApiError::operation),
                api.and_then(ApiError::status),
                SafeText::new(code, &[]),
                SafeText::new(&message, &[]),
                api.is_some_and(ApiError::retryable),
            ))
            .with_cause(error)
        } else {
            error
        };
        Failure::new(error, None, dispatch)
    }
}

fn observed_status(error: &Error) -> Option<StatusCode> {
    error
        .detail::<StatusCode>()
        .copied()
        .or_else(|| StatusCode::from_u16(error.api_error()?.status()?).ok())
}

fn is_rejected(error: &Error) -> bool {
    observed_status(error).is_some_and(|status| {
        status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT && status.as_u16() != 499
    })
}

fn same_vpcs(actual: &[String], expected: &[String]) -> bool {
    let mut actual: Vec<_> = actual.iter().collect();
    let mut expected: Vec<_> = expected.iter().collect();
    actual.sort_unstable();
    expected.sort_unstable();
    actual == expected
}

fn matches_update(record: &Record, zone: &HostedZone, input: &UpdateRecordInput) -> bool {
    input.sub_domain.as_ref().is_none_or(|label| {
        let full = if label.is_empty() {
            zone.domain_name.clone()
        } else {
            format!("{label}.{}", zone.domain_name)
        };
        record.sub_domain.eq_ignore_ascii_case(&full)
    }) && input
        .record_type
        .as_ref()
        .is_none_or(|kind| record.record_type.eq_ignore_ascii_case(kind))
        && input.ttl.is_none_or(|ttl| record.ttl == Field::Value(ttl))
        && input
            .routing_policy
            .as_ref()
            .is_none_or(|policy| record.routing_policy == *policy)
        && input
            .values
            .as_ref()
            .is_none_or(|values| match_values(&record.value, values, &record.record_type))
        && input
            .sticky_session
            .is_none_or(|flag| record.enable_sticky_session == Some(Field::Value(flag)))
}

fn match_values(actual: &[RecordValue], expected: &[RecordValue], kind: &str) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            (actual.value == expected.value
                || (kind == "MX" && actual.value.eq_ignore_ascii_case(&expected.value)))
                && actual.location == expected.location
                && actual.weight == expected.weight
        })
}
