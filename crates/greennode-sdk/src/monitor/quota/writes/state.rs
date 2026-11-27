mod metadata;

pub(super) use metadata::RetainedRedaction;

use std::{
    collections::BTreeMap,
    future::{Future, poll_fn},
    pin::pin,
    sync::Mutex,
    task::Poll,
    time::Duration,
};

use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::value::RawValue;
use time::OffsetDateTime;

use super::synthetic_settlement::Action;
use crate::monitor::mutation::{Mutation, Reply, SendGuard, screen_details};
use crate::monitor::wire::{Family, Read, input_error};
use crate::monitor::{Client, Quota, QuotaTimestamp, ResponseField};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::Screen;
use super::super::notification::NotificationKind;
use super::super::status::Detail;
use super::super::wire::Object;

pub(super) const NOTIFICATION_REDIRECT: &str =
    "https://vmonitor.console.vngcloud.vn/quota-usages/notification";
pub(super) const POLL: Duration = Duration::from_secs(2);
pub(super) const ORDER_BOUND: Duration = Duration::from_secs(120);
const CREDENTIAL_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Kind {
    Email,
    Sms,
    Synthetic,
}
impl From<NotificationKind> for Kind {
    fn from(kind: NotificationKind) -> Self {
        match kind {
            NotificationKind::Email => Self::Email,
            NotificationKind::Sms => Self::Sms,
        }
    }
}
impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Sms => "sms",
            Self::Synthetic => "synthetic",
        }
    }
    pub(super) fn current(self) -> &'static str {
        match self {
            Self::Email => "billing-api/v1/email/quota",
            Self::Sms => "billing-api/v1/sms/quota",
            Self::Synthetic => "billing-api/v1/synthetic/quota",
        }
    }
    pub(super) fn price(self) -> &'static str {
        match self {
            Self::Email => "billing-api/v2/email/prices/created-price",
            Self::Sms => "billing-api/v2/sms/prices/created-price",
            Self::Synthetic => "billing-api/v2/synthetic/prices/created-price",
        }
    }
    pub(super) fn renewal(self) -> &'static str {
        match self {
            Self::Email => "billing-api/v1/email/quota/{}/update-auto-renew",
            Self::Sms => "billing-api/v1/sms/quota/{}/update-auto-renew",
            Self::Synthetic => "billing-api/v1/synthetic/quota/{}/update-auto-renew",
        }
    }
    pub(super) fn absent(self) -> &'static str {
        match self {
            Self::Email => "Not found email quota",
            Self::Sms => "Not found sms quota",
            Self::Synthetic => "Not found synthetic quota",
        }
    }
    pub(super) fn packages(self) -> &'static str {
        match self {
            Self::Email => "billing-api/v1/email/packages",
            Self::Sms => "billing-api/v1/sms/packages",
            Self::Synthetic => "billing-api/v1/synthetic/packages",
        }
    }
}

#[derive(Default)]
pub(in crate::monitor) struct QuotaLedger {
    gate: tokio::sync::Mutex<()>,
    pending: Mutex<BTreeMap<Kind, Attempt>>,
}
impl QuotaLedger {
    pub(super) async fn acquire<'a>(
        &'a self,
        ctx: &RequestContext,
        clock: &dyn crate::Clock,
    ) -> Result<tokio::sync::MutexGuard<'a, ()>, Error> {
        if let Some(end) = ctx.check(clock) {
            return Err(end.into());
        }
        let mut stopped = pin!(ctx.wait(clock));
        let mut acquired = pin!(self.gate.lock());
        let guard = poll_fn(|cx| {
            if let Poll::Ready(end) = stopped.as_mut().poll(cx) {
                return Poll::Ready(Err(Error::from(end)));
            }
            acquired.as_mut().poll(cx).map(Ok)
        })
        .await?;
        if let Some(end) = ctx.check(clock) {
            return Err(end.into());
        }
        Ok(guard)
    }
    pub(super) fn get(&self, kind: Kind) -> Result<Option<Attempt>, Error> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| Error::new(ErrorClass::Api))?;
        Ok(pending.get(&kind).map(Attempt::copy))
    }
    pub(super) fn put(&self, attempt: &Attempt) -> Result<(), Error> {
        self.pending
            .lock()
            .map_err(|_| Error::new(ErrorClass::Api))?
            .insert(attempt.kind, attempt.copy());
        Ok(())
    }
    pub(super) fn observe(
        &self,
        kind: Kind,
        credentials: &crate::wire::product::evidence::SentCredentials,
    ) -> Result<(), Error> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| Error::new(ErrorClass::Api))?;
        if let Some(attempt) = pending.get_mut(&kind) {
            attempt.retain_credentials(credentials);
        }
        Ok(())
    }

    pub(super) fn clear(&self, kind: Kind) -> Result<(), Error> {
        self.pending
            .lock()
            .map_err(|_| Error::new(ErrorClass::Api))?
            .remove(&kind);
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub(super) quota: Quota,
    pub(super) details: Option<Details>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Details {
    pub(super) class_id: String,
    pub(super) api: i64,
    pub(super) location: i64,
    pub(super) price: Option<f64>,
}
impl From<Detail> for Details {
    fn from(detail: Detail) -> Self {
        Self {
            class_id: detail.class_id,
            api: detail.api,
            location: detail.location,
            price: detail.price,
        }
    }
}
impl Snapshot {
    pub(super) fn id(&self) -> &str {
        &self.quota.id
    }
    pub(super) fn package(&self) -> &str {
        &self.quota.package_id
    }
    pub(super) fn status(&self) -> &str {
        &self.quota.status
    }
    pub(super) fn end(&self) -> Option<OffsetDateTime> {
        field(self.quota.end_time).map(|value| value.0)
    }
    pub(super) fn start(&self) -> Option<OffsetDateTime> {
        field(self.quota.start_time).map(|value| value.0)
    }
    pub(super) fn same(&self, other: &Self) -> bool {
        let x = &self.quota;
        let y = &other.quota;
        x.id == y.id
            && x.package_id == y.package_id
            && x.status == y.status
            && x.end_time == y.end_time
            && x.auto_renew == y.auto_renew
            && x.auto_renew_period == y.auto_renew_period
            && x.poc == y.poc
            && self.details == other.details
    }
}
pub(super) fn field<T: Copy>(value: ResponseField<T>) -> Option<T> {
    match value {
        ResponseField::Value(value) => Some(value),
        ResponseField::Redacted => None,
    }
}

#[derive(Clone)]
pub(super) enum Goal {
    Notification {
        package: String,
    },
    RenewSetting {
        before: Snapshot,
        enabled: bool,
    },
    Delete {
        before: Snapshot,
    },
    SyntheticCreate {
        class_id: String,
        package: String,
        quantity: i64,
        location: i64,
        sent_at: QuotaTimestamp,
    },
    SyntheticChange {
        before: Snapshot,
        action: Action,
    },
}
impl Goal {
    pub(super) fn id(&self) -> &str {
        self.before().map_or("", Snapshot::id)
    }
    pub(super) fn before(&self) -> Option<&Snapshot> {
        match self {
            Self::RenewSetting { before, .. }
            | Self::Delete { before }
            | Self::SyntheticChange { before, .. } => Some(before),
            _ => None,
        }
    }
    pub(super) fn bound(&self) -> Duration {
        match self {
            Self::Notification { .. } => Duration::ZERO,
            Self::RenewSetting { .. } => Duration::from_secs(30),
            Self::Delete { .. } => Duration::from_secs(60),
            Self::SyntheticCreate { .. } | Self::SyntheticChange { .. } => ORDER_BOUND,
        }
    }
}
pub(super) struct Attempt {
    pub(super) op: &'static str,
    pub(super) kind: Kind,
    pub(super) goal: Goal,
    pub(super) order_id: String,
    pub(super) payment_pending: bool,
    pub(super) observed_status: u16,
    pub(super) evidence: Evidence,
    pub(super) credentials_exhausted: bool,
    pub(super) retained_redaction: RetainedRedaction,
}
impl Attempt {
    pub(super) fn new(op: &'static str, kind: Kind, goal: Goal, evidence: Evidence) -> Self {
        Self {
            op,
            kind,
            goal,
            order_id: String::new(),
            payment_pending: false,
            observed_status: 0,
            evidence,
            credentials_exhausted: false,
            retained_redaction: RetainedRedaction::default(),
        }
    }
    pub(super) fn sent(&self) -> &'static str {
        if self.observed_status == 0 {
            "unknown"
        } else {
            "yes"
        }
    }
    fn retain_credentials(
        &mut self,
        credentials: &crate::wire::product::evidence::SentCredentials,
    ) {
        let mut dropped = Evidence::new();
        if !self
            .evidence
            .absorb_bounded(credentials, CREDENTIAL_LIMIT, |secret| {
                dropped.add_secret(secret)
            })
        {
            self.credentials_exhausted = true;
            if !self.retained_redaction.observe(&dropped) {
                self.evidence.withhold_partial();
            }
        }
        if self.credentials_exhausted {
            self.order_id.clear();
        }
    }

    pub(super) fn merge_retained(&mut self, retained: &Self) {
        let evidence = std::mem::take(&mut self.evidence);
        self.credentials_exhausted |= retained.credentials_exhausted;
        self.retained_redaction.merge(&retained.retained_redaction);
        self.retain_credentials(retained.evidence.credentials());
        self.retain_credentials(evidence.credentials());
    }

    fn copy(&self) -> Self {
        let mut copy = Self {
            op: self.op,
            kind: self.kind,
            goal: self.goal.clone(),
            order_id: self.order_id.clone(),
            payment_pending: self.payment_pending,
            observed_status: self.observed_status,
            evidence: Evidence::new(),
            credentials_exhausted: self.credentials_exhausted,
            retained_redaction: self.retained_redaction,
        };
        copy.retain_credentials(self.evidence.credentials());
        copy
    }
}

pub(super) enum SendState {
    Refused {
        error: Error,
        dispatch: WriteDispatch,
        status: u16,
    },
    Pending {
        failure: Option<Error>,
        reply: Option<Reply>,
    },
}
impl Client {
    pub(super) async fn send_quota(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        path: &[&str],
        body: Option<&serde_json::Value>,
        ids: &[&str],
        attempt: &mut Attempt,
    ) -> Result<SendState, Error> {
        let mut cancellation = PendingSend {
            ledger: &self.quota_ledger,
            attempt: attempt.copy(),
            dispatch: Default::default(),
            completed: false,
        };
        let data = body.map(|value| value as &dyn crate::monitor::mutation::SerializeBody);
        let sent = self
            .mutate_guarded::<()>(
                ctx,
                mutation,
                path,
                data,
                &mut attempt.evidence,
                SendGuard {
                    ids,
                    dispatch: Some(&cancellation.dispatch),
                    privacy: None,
                },
            )
            .await;
        cancellation.completed = true;
        match sent {
            Ok(reply) => {
                attempt.observed_status = reply.status;
                self.quota_ledger.put(attempt)?;
                Ok(SendState::Pending {
                    failure: None,
                    reply: Some(reply),
                })
            }
            Err(failure) => {
                let (error, _, dispatch) = failure.into_parts();
                let observed = error.detail::<StatusCode>().map(StatusCode::as_u16);
                let api_status = error.api_error().and_then(crate::ApiError::status);
                let status = observed.or(api_status).unwrap_or_default();
                let overflow = error.is_body_too_large();
                let incomplete = !mutation.keep_status
                    && !overflow
                    && observed.is_some()
                    && api_status.is_none();
                let error = if overflow {
                    mutation
                        .error(
                            (status != 0).then_some(status),
                            "ResponseTooLarge",
                            "quota response exceeded size limit",
                            &attempt.evidence,
                        )
                        .with_cause(error)
                } else if let Some(api) = error.api_error().filter(|_| status > 0) {
                    let read = Read {
                        id: mutation.id,
                        route: mutation.route,
                        limit: mutation.limit,
                        family: Family::Quota {
                            no_redirect: mutation.no_redirect,
                        },
                    };
                    let withheld = read.error(
                        (!incomplete).then_some(status),
                        if status < 400 || incomplete {
                            "InvalidResponse"
                        } else {
                            ""
                        },
                        "quota response withheld",
                        api.retryable(),
                        &attempt.evidence,
                    );
                    if api.message() == "quota response withheld" {
                        error
                    } else {
                        withheld.with_cause(error)
                    }
                } else {
                    error
                };
                if dispatch == WriteDispatch::NotSent
                    || (!incomplete && (400..500).contains(&status))
                {
                    self.quota_ledger.clear(attempt.kind)?;
                    return Ok(SendState::Refused {
                        error,
                        dispatch,
                        status,
                    });
                }
                attempt.observed_status = status;
                self.quota_ledger.put(attempt)?;
                Ok(SendState::Pending {
                    failure: Some(error),
                    reply: None,
                })
            }
        }
    }
}

struct PendingSend<'a> {
    ledger: &'a QuotaLedger,
    attempt: Attempt,
    dispatch: crate::wire::product::options::DispatchRecord,
    completed: bool,
}

impl Drop for PendingSend<'_> {
    fn drop(&mut self) {
        if !self.completed
            && let Some(dispatched) = self.dispatch.take()
        {
            self.attempt.evidence.absorb(&dispatched.credentials);
            self.attempt.observed_status = dispatched
                .status
                .map(|status| status.as_u16())
                .unwrap_or_default();
            let _ = self.ledger.put(&self.attempt);
        }
    }
}

pub(super) enum Observation {
    Absent(Error),
    Present(Snapshot),
}
impl Observation {
    pub(super) fn present(self) -> Result<Snapshot, Error> {
        match self {
            Self::Present(snapshot) => Ok(snapshot),
            Self::Absent(error) => Err(error),
        }
    }
}

pub(super) fn mutation(
    op: &'static str,
    route: &'static str,
    method: &'static Method,
    success: &'static [StatusCode],
    keep_status: bool,
) -> Mutation {
    Mutation {
        success,
        keep_status,
        limit: 1 << 20,
        ..Mutation::standard(op, route, method)
    }
}
pub(super) fn unsettled(op: &'static str, cause: Option<Error>) -> Error {
    let error=Error::new(ErrorClass::NotSettled).with_outer_detail(format!("{op}: quota attempt remains unresolved; reconcile reads and inspect billing before another write"));
    match cause {
        Some(cause) => error.with_cause(cause.with_api_operation(op)),
        None => error,
    }
}
pub(super) fn invalid(op: &str, description: &str) -> Error {
    input_error(format!("{op}: {description}: vngcloud: invalid input"))
}
pub(super) fn price_guard(
    op: &'static str,
    evidence: &Evidence,
    list: f64,
    quote: f64,
    cap: f64,
) -> Result<(), Error> {
    if !list.is_finite() || !quote.is_finite() || list < 0.0 || quote < 0.0 {
        return Err(
            mutation(op, "", &Method::POST, &[StatusCode::OK], false).invalid(200, evidence)
        );
    }
    if list > 0.0 && quote == 0.0 {
        return Err(Error::new(ErrorClass::Unpriced).with_outer_detail(format!("{op}: vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource")));
    }
    let guard = list.max(quote);
    if guard > cap {
        return Err(Error::new(ErrorClass::PriceAboveMax)
            .with_outer_detail(format!("{op}: vngcloud: quoted price is above MaxPrice")));
    }
    Ok(())
}
pub(super) fn validate_cap(op: &str, cap: f64) -> Result<(), Error> {
    if !cap.is_finite() || cap < 0.0 {
        return Err(invalid(op, "MaxPrice must be finite and nonnegative"));
    }
    Ok(())
}
pub(super) fn validate_profile(client: &Client, op: &'static str) -> Result<(), Error> {
    match client
        .quota_read_only
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "0" | "false" => Ok(()),
        "1" | "true" => Err(Error::new(ErrorClass::InvalidInput).with_cause(
            mutation(op, "", &Method::POST, &[StatusCode::OK], false).error(
                None,
                "ReadOnly",
                "read-only profile refuses quota writes",
                &Evidence::new(),
            ),
        )),
        _ => Err(
            Error::new(ErrorClass::InvalidConfig).with_outer_detail(format!(
                "{op}: vngcloud: invalid config: read_only must be 1, true, 0, false, or empty"
            )),
        ),
    }
}
pub(super) fn decode_order(
    reply: &Reply,
    mutation: &Mutation,
    evidence: &Evidence,
    redirect: &str,
    allow_empty: bool,
) -> Result<(String, bool), Error> {
    const SHAPE: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "orderId",
            shape: Shape::Any,
        },
        ShapeField {
            name: "paymentUrl",
            shape: Shape::Any,
        },
    ]);
    #[derive(Deserialize)]
    struct Ack {
        #[serde(rename = "orderId")]
        id: Option<String>,
        #[serde(rename = "paymentUrl")]
        payment: Option<String>,
    }
    if allow_empty && reply.is_empty() {
        return Ok((String::new(), false));
    }
    let coded: Box<RawValue> = reply.decode(mutation, &Shape::Any, evidence)?;
    let raw = coded.get();
    if allow_empty && raw == "null" {
        return Ok((String::new(), false));
    }
    crate::wire::strict_json::validate_canonical_shape(raw.as_bytes(), &SHAPE, raw.len())
        .map_err(|_| mutation.invalid(reply.status, evidence))?;
    let object = Object::of(&coded).ok_or_else(|| mutation.invalid(reply.status, evidence))?;
    if !allow_empty
        && (object.required("orderId").is_none() || object.required("paymentUrl").is_none())
    {
        return Err(mutation.invalid(reply.status, evidence));
    }
    let ack: Ack =
        serde_json::from_str(raw).map_err(|_| mutation.invalid(reply.status, evidence))?;
    let id = ack.id.unwrap_or_default();
    if !id.is_empty() && !evidence.id_reflects(&id) && !PathIds::WithUnderscore.allows(&id) {
        return Err(mutation.invalid(reply.status, evidence));
    }
    Ok((
        id,
        ack.payment
            .is_some_and(|payment| !payment.is_empty() && payment != redirect),
    ))
}
pub(super) fn result<T: Screen>(
    mut value: T,
    result: Result<(), Error>,
    dispatch: WriteDispatch,
    evidence: &Evidence,
) -> Result<T, Failure<T>> {
    let retain = evidence.credentials().values().len() <= CREDENTIAL_LIMIT
        && evidence.can_retain_partial()
        && value.screen(evidence);
    match result {
        Ok(()) if retain => Ok(value),
        Ok(()) => Err(Failure::new(
            Error::new(ErrorClass::Api).with_outer_detail(
                "quota response withheld because credential safety could not be confirmed"
                    .to_owned(),
            ),
            None,
            dispatch,
        )),
        Err(error) => Err(Failure::new(
            screen_details(error, evidence),
            retain.then_some(value),
            dispatch,
        )),
    }
}
pub(super) fn screen_quota(quota: &mut Option<Quota>, evidence: &Evidence) {
    if let Some(quota) = quota {
        for id in [&mut quota.id, &mut quota.package_id] {
            if evidence.id_reflects(id) {
                *id = "<redacted>".into();
            }
        }
    }
}
pub(super) fn order_display(value: &str, evidence: &Evidence) -> String {
    if evidence.id_reflects(value) {
        "<redacted>".into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests;
