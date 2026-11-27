use super::super::notification::notification_kind;
use super::super::output;
use super::super::status::notification_packages;
use super::super::wire::Reader;
use super::lifecycle::changed_intent;
use super::state::{
    Attempt, Goal, Kind, NOTIFICATION_REDIRECT, ORDER_BOUND, Observation, SendState, decode_order,
    invalid, mutation, order_display, price_guard, quota_error, result, screen_quota, unsettled,
    validate_cap,
};
use crate::monitor::mutation::not_sent;
use crate::monitor::{Client, CreateNotificationQuotaInput, Quota};
use crate::quote::request::{self, Product, Request};
use crate::wire::product::evidence::Evidence;
use crate::{Error, Failure, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::HashSet;
const CREATE: &str = "monitor.CreateNotificationQuota";
const OK: &[StatusCode] = &[StatusCode::OK];

output! {
    /// Result of ordering one month of notification quota.
    struct CreateNotificationQuotaOutput {
        /// Current quota, when confirmed by a read.
        quota:Option<Quota> =>"Quota",
        /// Order identifier, when supplied by the billing endpoint.
        order_id:String=>"OrderID",
        /// Whether billing still reports a pending payment.
        payment_pending:bool=>"PaymentPending",
    }
}

impl Client {
    /// Prices and orders one month of notification quota, then confirms it unless NoWait is set.
    pub async fn create_notification_quota(
        &self,
        ctx: &RequestContext,
        input: CreateNotificationQuotaInput,
    ) -> Result<CreateNotificationQuotaOutput, Failure<CreateNotificationQuotaOutput>> {
        let kind = Kind::from(notification_kind(CREATE, &input.kind).map_err(not_sent)?);
        validate_cap(CREATE, input.max_price).map_err(not_sent)?;
        let _gate = self
            .quota_ledger
            .acquire(ctx, self.session.clock())
            .await
            .map_err(not_sent)?;
        if let Some(mut attempt) = self.quota_ledger.get(kind).map_err(not_sent)? {
            let matched = attempt.op == CREATE
                && matches!(&attempt.goal,Goal::Notification{package} if input.package_id.is_empty()||*package==input.package_id);
            let bound = attempt.goal.bound();
            let pending = self.reconcile_quota(ctx, &mut attempt, CREATE, bound).await;
            return match pending {
                Ok(quota) if matched => notification_result(&attempt, quota, Ok(()), false),
                Ok(_) => Err(not_sent(quota_error(
                    changed_intent(CREATE),
                    &attempt.evidence,
                ))),
                Err(error) if matched => {
                    notification_result(&attempt, None, Err(error), attempt.payment_pending)
                }
                Err(error) => Err(Failure::new(
                    quota_error(error, &attempt.evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )),
            };
        }
        let mut evidence = self
            .write_evidence(ctx, &[&input.package_id])
            .map_err(not_sent)?;
        self.require_absent_quota(ctx, kind, CREATE, &mut evidence)
            .await
            .map_err(not_sent)?;
        let (package, listed) = self
            .notification_package(ctx, kind, &input.package_id, &mut evidence)
            .await
            .map_err(not_sent)?;
        let body = json!({"redirectUrl":"","packageId":package,"quantity":1});
        let quoted = self
            .quota_offer_price(ctx, CREATE, kind, &[&package], &body, &mut evidence)
            .await
            .map_err(not_sent)?;
        price_guard(CREATE, &evidence, listed, quoted, input.max_price).map_err(not_sent)?;
        self.require_absent_quota(ctx, kind, CREATE, &mut evidence)
            .await
            .map_err(not_sent)?;
        let order = json!({"monthPeriod":1,"packageId":package,"redirectUrl":NOTIFICATION_REDIRECT,"pay":true});
        let request = mutation(CREATE, kind.current(), &Method::POST, OK, false);
        let mut attempt = Attempt::new(
            CREATE,
            kind,
            Goal::Notification {
                package: package.clone(),
            },
            evidence,
        );
        let send = self
            .send_quota(ctx, &request, &[], Some(&order), &[&package], &mut attempt)
            .await
            .map_err(not_sent)?;
        match send {
            SendState::Refused {
                error, dispatch, ..
            } => return Err(Failure::new(error, None, dispatch)),
            SendState::Pending {
                failure: Some(error),
                ..
            } => {
                return notification_result(
                    &attempt,
                    None,
                    Err(unsettled(CREATE, Some(error))),
                    false,
                );
            }
            SendState::Pending {
                reply: Some(reply), ..
            } => {
                let ack = decode_order(
                    &reply,
                    &request,
                    &attempt.evidence,
                    NOTIFICATION_REDIRECT,
                    false,
                );
                match ack {
                    Ok((order, payment)) => {
                        attempt.order_id = order;
                        attempt.payment_pending = payment;
                    }
                    Err(error) => {
                        return notification_result(
                            &attempt,
                            None,
                            Err(unsettled(CREATE, Some(error))),
                            false,
                        );
                    }
                }
            }
            SendState::Pending { .. } => {
                return notification_result(&attempt, None, Err(unsettled(CREATE, None)), false);
            }
        }
        self.quota_ledger
            .put(&attempt)
            .map_err(|error| Failure::new(error, None, WriteDispatch::MayHaveBeenSent))?;
        if attempt.payment_pending {
            return notification_result(&attempt, None, Err(unsettled(CREATE, None)), true);
        }
        if input.no_wait {
            return notification_result(&attempt, None, Ok(()), false);
        }
        match self
            .reconcile_quota(ctx, &mut attempt, CREATE, ORDER_BOUND)
            .await
        {
            Ok(quota) => notification_result(&attempt, quota, Ok(()), false),
            Err(error) => notification_result(&attempt, None, Err(error), false),
        }
    }

    pub(super) async fn require_absent_quota(
        &self,
        ctx: &RequestContext,
        kind: Kind,
        op: &'static str,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        match self
            .observe_quota(ctx, kind, op, evidence, false, true)
            .await?
        {
            Observation::Absent(_) => Ok(()),
            Observation::Present(_) => Err(invalid(
                op,
                if kind == Kind::Synthetic {
                    "synthetic quota already exists"
                } else {
                    "a notification quota already exists"
                },
            )),
        }
    }
    async fn notification_package(
        &self,
        ctx: &RequestContext,
        kind: Kind,
        requested: &str,
        evidence: &mut Evidence,
    ) -> Result<(String, f64), Error> {
        let mut reader = Reader::new(self, ctx, CREATE);
        reader.read.family = crate::monitor::wire::Family::Quota { no_redirect: false };
        reader.evidence.absorb(evidence.credentials());
        let answer = async {
            let raw = reader.get(kind.packages(), &[]).await?;
            let list = reader.decode(notification_packages(&raw))?;
            let mut seen = HashSet::new();
            let mut selected = None;
            for item in &list {
                if reader.evidence.id_reflects(&item.id) {
                    return Err(reader.invalid());
                }
                if !seen.insert(&item.id) {
                    return Err(invalid(CREATE, "duplicate package IDs"));
                }
                let matched = if requested.is_empty() {
                    item.price == Some(0.0)
                } else {
                    item.id == requested
                };
                if matched && selected.replace(item).is_some() {
                    return Err(invalid(CREATE, "ambiguous free package selection"));
                }
            }
            let package = selected.ok_or_else(|| invalid(CREATE, "no matching package"))?;
            Ok((package.id.clone(), reader.decode(package.price)?))
        }
        .await;
        evidence.absorb(reader.evidence.credentials());
        answer
    }
    pub(super) async fn quota_offer_price(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        kind: Kind,
        ids: &[&str],
        body: &Value,
        evidence: &mut Evidence,
    ) -> Result<f64, Error> {
        let raw = request::exchange(
            &self.session,
            ctx,
            evidence,
            Request {
                operation: op,
                template: kind.price(),
                product: Product::Monitor {
                    identifiers: ids,
                    no_redirect: op == "monitor.CreateSyntheticQuota",
                },
                path: &[],
                query: &[],
                body: Some(body),
            },
            None,
        )
        .await
        .map_err(|error| *error)?;
        let price = request::monitor::price(&raw)
            .ok_or_else(|| mutation(op, "", &Method::POST, OK, false).invalid(200, evidence))?;
        if ids.iter().any(|id| evidence.id_reflects(id)) || quota_price_reflects(price, evidence) {
            return Err(mutation(op, "", &Method::POST, OK, false).invalid(200, evidence));
        }
        Ok(price)
    }
}

fn quota_price_reflects(price: f64, evidence: &Evidence) -> bool {
    use crate::output_number::{Style, f64};
    [Style::Json, Style::Fixed]
        .into_iter()
        .any(|style| f64(price, style).is_none_or(|text| evidence.text_reflects(&text)))
}
fn notification_result(
    attempt: &Attempt,
    mut quota: Option<Quota>,
    outcome: Result<(), Error>,
    payment_pending: bool,
) -> Result<CreateNotificationQuotaOutput, Failure<CreateNotificationQuotaOutput>> {
    screen_quota(&mut quota, &attempt.evidence);
    result(
        CreateNotificationQuotaOutput {
            quota,
            order_id: order_display(&attempt.order_id, &attempt.evidence),
            payment_pending,
        },
        outcome,
        WriteDispatch::MayHaveBeenSent,
        &attempt.evidence,
    )
}
