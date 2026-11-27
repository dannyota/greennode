mod model;
mod spec;

use http::{Method, StatusCode};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, UtcOffset};

use crate::monitor::{Client, CreateSyntheticQuotaInput, QuotaTimestamp, RenewSyntheticQuotaInput};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::Screen;
use super::super::catalog::{OfferIssue, QuoteOffer, quote_offer};
use super::super::wire::Reader;
use super::lifecycle::changed_intent;
use super::state::{
    Attempt, Goal, Kind, ORDER_BOUND, SendState, Snapshot, decode_order, invalid, mutation,
    price_guard, result, unsettled, validate_profile,
};
use super::synthetic_settlement::Action;
use model::Draft;
use spec::{Intent, Spec};

pub use model::{
    CreateSyntheticQuotaOutput, EnsureSyntheticQuotaInput, EnsureSyntheticQuotaOutput,
    QuotaMutationOutcome, QuotaOutcomeError, RecoverSyntheticQuotaInput,
    RecoverSyntheticQuotaOutput, RenewSyntheticQuotaOutput,
};

const SYNTHETIC_REDIRECT: &str = "https://vmonitor.console.vngcloud.vn/quota-usages/synthetic-test";

const CREATE: &str = "monitor.CreateSyntheticQuota";
const RENEW: &str = "monitor.RenewSyntheticQuota";
const RECOVER: &str = "monitor.RecoverSyntheticQuota";
const ENSURE: &str = "monitor.EnsureSyntheticQuota";
const OK: &[StatusCode] = &[StatusCode::OK];
impl Client {
    /// Orders the selected free synthetic quota once and confirms it by reading.
    pub async fn create_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: CreateSyntheticQuotaInput,
    ) -> Result<CreateSyntheticQuotaOutput, Failure<CreateSyntheticQuotaOutput>> {
        self.run_synthetic(
            ctx,
            Spec {
                op: CREATE,
                intent: Intent::Create {
                    class: &input.class_id,
                    package: &input.package_id,
                    quantity: input.quantity,
                },
                cap: input.max_price,
                days: 0,
                no_wait: input.no_wait,
            },
            Draft::create,
        )
        .await
    }
    /// Renews the selected active free synthetic quota once.
    pub async fn renew_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: RenewSyntheticQuotaInput,
    ) -> Result<RenewSyntheticQuotaOutput, Failure<RenewSyntheticQuotaOutput>> {
        self.run_synthetic(
            ctx,
            Spec {
                op: RENEW,
                intent: Intent::Renew {
                    end: &input.expected_end_time,
                },
                cap: input.max_price,
                days: input.max_days_left,
                no_wait: input.no_wait,
            },
            Draft::renew,
        )
        .await
    }
    /// Recovers the selected expired free synthetic quota from trash once.
    pub async fn recover_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: RecoverSyntheticQuotaInput,
    ) -> Result<RecoverSyntheticQuotaOutput, Failure<RecoverSyntheticQuotaOutput>> {
        self.run_synthetic(
            ctx,
            Spec {
                op: RECOVER,
                intent: Intent::Recover {
                    id: &input.expected_quota_id,
                },
                cap: input.max_price,
                days: 0,
                no_wait: input.no_wait,
            },
            Draft::recover,
        )
        .await
    }
    /// Selects renewal or recovery from the current quota state and performs at most one mutation.
    pub async fn ensure_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: EnsureSyntheticQuotaInput,
    ) -> Result<EnsureSyntheticQuotaOutput, Failure<EnsureSyntheticQuotaOutput>> {
        self.run_synthetic(
            ctx,
            Spec {
                op: ENSURE,
                intent: Intent::Ensure {
                    id: &input.expected_quota_id,
                },
                cap: input.max_price,
                days: input.max_days_left,
                no_wait: input.no_wait,
            },
            Draft::ensure,
        )
        .await
    }

    async fn run_synthetic<T: Screen>(
        &self,
        ctx: &RequestContext,
        spec: Spec<'_>,
        make: fn(Draft, QuotaMutationOutcome) -> T,
    ) -> Result<T, Failure<T>> {
        let mut state = Run::default();
        let outcome = self
            .synthetic_inner(ctx, &spec, &mut state)
            .await
            .map_err(|error| {
                if state.draft.sent != "no" {
                    return error;
                }
                let Ok(Some(attempt)) = self.quota_ledger.get(Kind::Synthetic) else {
                    return error;
                };
                state.adopt(&attempt);
                state.draft.order_id.clear();
                state.draft.payment_pending = false;
                let cause = if spec.early_guard_mismatch(&attempt.goal) {
                    error.with_cause(changed(spec.op))
                } else {
                    error
                };
                unsettled(spec.op, Some(cause))
            });
        let login_evidence = if state.dispatch == WriteDispatch::NotSent
            && outcome.as_ref().is_err_and(|error| {
                error.service_account_login_error().is_some() || error.login_error().is_some()
            }) {
            state.evidence.for_first_login_preflight()
        } else {
            None
        };
        if login_evidence.is_some() {
            state.draft = Draft::default();
        }
        let evidence = login_evidence.as_ref().unwrap_or(&state.evidence);
        let output = state
            .draft
            .project(spec.op, outcome.as_ref().err(), evidence, make);
        result(output, outcome, state.dispatch, evidence)
    }

    async fn synthetic_inner(
        &self,
        ctx: &RequestContext,
        spec: &Spec<'_>,
        state: &mut Run,
    ) -> Result<(), Error> {
        state.expected_end = spec.validate()?;
        validate_profile(self, spec.op)?;
        let _gate = self.quota_ledger.acquire(ctx, self.session.clock()).await?;
        if let Some(mut pending) = self.quota_ledger.get(Kind::Synthetic)? {
            state.adopt(&pending);
            let mismatch = spec.guard_mismatch(&pending.goal, state.expected_end);
            let matches = spec.matches(&pending, state.expected_end);
            let bound = if matches!(spec.intent, Intent::Create { .. }) {
                pending.goal.bound()
            } else {
                ORDER_BOUND
            };
            match self
                .reconcile_quota(ctx, &mut pending, spec.op, bound)
                .await
            {
                Ok(quota) if matches => {
                    state.confirm(&pending, quota);
                    return Ok(());
                }
                Ok(_) => {
                    state.evidence.absorb(pending.evidence.credentials());
                    state.reset();
                    return Err(
                        if mismatch && !matches!(spec.intent, Intent::Create { .. }) {
                            changed(spec.op)
                        } else {
                            changed_intent(spec.op)
                        },
                    );
                }
                Err(error) => {
                    state.adopt(&pending);
                    return Err(
                        if mismatch && !matches!(spec.intent, Intent::Create { .. }) {
                            error.with_cause(changed(spec.op))
                        } else {
                            error
                        },
                    );
                }
            }
        }
        let operands = match spec.intent {
            Intent::Create { class, package, .. } => [class, package],
            Intent::Renew { end } => [end, ""],
            Intent::Recover { id } | Intent::Ensure { id } => [id, ""],
        };
        state.evidence = self.write_evidence(ctx, &operands)?;
        let prepared = match spec.intent {
            Intent::Create {
                class,
                package,
                quantity,
            } => {
                self.prepare_synthetic_create(ctx, spec, class, package, quantity, state)
                    .await?
            }
            _ => match self.prepare_synthetic_change(ctx, spec, state).await? {
                Some(value) => value,
                None => return Ok(()),
            },
        };
        self.send_synthetic(ctx, spec, prepared, state).await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "The three required create operands remain explicit until resolved"
    )]
    async fn prepare_synthetic_create(
        &self,
        ctx: &RequestContext,
        spec: &Spec<'_>,
        class: &str,
        package: &str,
        quantity: i64,
        state: &mut Run,
    ) -> Result<Prepared, Error> {
        self.require_absent_quota(ctx, Kind::Synthetic, spec.op, &mut state.evidence)
            .await?;
        let offer = self
            .synthetic_offer(ctx, spec.op, class, package, quantity, &mut state.evidence)
            .await?;
        let quote_body =
            json!({"redirectUrl":"","packageId":package,"quantity":quantity,"buyWith":{}});
        let quote = self
            .quota_offer_price(
                ctx,
                spec.op,
                Kind::Synthetic,
                &[class, package],
                &quote_body,
                &mut state.evidence,
            )
            .await?;
        free_price(spec, &state.evidence, offer.list_price, quote)?;
        let fresh = final_guard(
            self.synthetic_offer(ctx, spec.op, class, package, quantity, &mut state.evidence)
                .await,
            spec.op,
        )?;
        if offer != fresh {
            return Err(changed(spec.op));
        }
        final_guard(
            self.require_absent_quota(ctx, Kind::Synthetic, spec.op, &mut state.evidence)
                .await,
            spec.op,
        )?;
        if let Some(end) = ctx.check(self.session.clock()) {
            return Err(end.into());
        }
        let sent_at = QuotaTimestamp(self.session.clock().utc_now().to_offset(UtcOffset::UTC));
        state.draft.sent_at = Some(sent_at);
        Ok(Prepared {
            goal: Goal::SyntheticCreate {
                class_id: class.into(),
                package: package.into(),
                quantity,
                location: offer.location,
                sent_at,
            },
            path: "billing-api/v2/synthetic/quota",
            ids: vec![class.into(), package.into()],
            body: json!({"redirectUrl":SYNTHETIC_REDIRECT,"packageId":package,"quantity":quantity,"buyWith":{},"monthPeriod":1,"pay":true}),
        })
    }

    async fn prepare_synthetic_change(
        &self,
        ctx: &RequestContext,
        spec: &Spec<'_>,
        state: &mut Run,
    ) -> Result<Option<Prepared>, Error> {
        let before = self
            .observe_quota(
                ctx,
                Kind::Synthetic,
                spec.op,
                &mut state.evidence,
                true,
                false,
            )
            .await?
            .present()?;
        let now = self.session.clock().utc_now();
        let action = spec.action(&before, now)?;
        state.draft.previous = before.end().map(QuotaTimestamp);
        spec.eligible(&before, state.expected_end, action, now)?;
        let detail = details(spec.op, &before, &state.evidence)?;
        let offer = self
            .synthetic_offer(
                ctx,
                spec.op,
                &detail.class_id,
                before.package(),
                detail.api,
                &mut state.evidence,
            )
            .await?;
        if offer.location != detail.location {
            return Err(invalid(spec.op, "synthetic location is unsupported"));
        }
        if action == Action::Renew
            && detail.price == Some(0.0)
            && offer.list_price == 0.0
            && spec.days > 0
            && self
                .session
                .clock()
                .utc_now()
                .checked_add(Duration::days(spec.days))
                .zip(before.end())
                .is_some_and(|(due, end)| end > due)
        {
            state.draft.stage = "not-due";
            return Ok(None);
        }
        state.draft.action = action;
        let quote_body = json!({"redirectUrl":"","packageId":before.package(),"quantity":detail.api,"resourceId":before.id()});
        let quote = self
            .quota_offer_price(
                ctx,
                spec.op,
                Kind::Synthetic,
                &[before.id(), before.package(), &detail.class_id],
                &quote_body,
                &mut state.evidence,
            )
            .await?;
        let listed = detail
            .price
            .ok_or_else(|| quota_invalid(spec.op, &state.evidence))?
            .max(offer.list_price);
        free_price(spec, &state.evidence, listed, quote)?;
        let fresh_catalog = final_guard(
            self.synthetic_offer(
                ctx,
                spec.op,
                &detail.class_id,
                before.package(),
                detail.api,
                &mut state.evidence,
            )
            .await,
            spec.op,
        )?;
        let fresh = self
            .observe_quota(
                ctx,
                Kind::Synthetic,
                spec.op,
                &mut state.evidence,
                true,
                false,
            )
            .await?
            .present()?;
        let checked = spec.eligible(
            &fresh,
            state.expected_end,
            action,
            self.session.clock().utc_now(),
        );
        if let Err(error) = &checked
            && error.api_error().is_some()
        {
            return checked.map(|()| None);
        }
        if checked.is_err()
            || offer != fresh_catalog
            || !before.same(&fresh)
            || fresh.details.as_ref().and_then(|detail| detail.price) != Some(0.0)
        {
            return Err(changed(spec.op));
        }
        let ids = vec![
            before.id().into(),
            before.package().into(),
            detail.class_id.clone(),
        ];
        let path = if action == Action::Recover {
            "billing-api/v2/trash/synthetic/quota"
        } else {
            "billing-api/v2/synthetic/quota/extend-time"
        };
        Ok(Some(Prepared {
            goal: Goal::SyntheticChange { before, action },
            path,
            ids,
            body: json!({"redirectUrl":SYNTHETIC_REDIRECT,"monthPeriod":1,"pay":true}),
        }))
    }

    async fn synthetic_offer(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        class: &str,
        package: &str,
        quantity: i64,
        evidence: &mut Evidence,
    ) -> Result<QuoteOffer, Error> {
        let mut reader = Reader::new(self, ctx, op);
        reader.evidence.absorb(evidence.credentials());
        let result = quote_offer(&mut reader, class, package, quantity).await;
        evidence.absorb(reader.evidence.credentials());
        result?.map_err(|issue| {
            invalid(
                op,
                match issue {
                    OfferIssue::Class => "selected synthetic class is unsupported",
                    OfferIssue::Package => "selected synthetic package is unsupported",
                    OfferIssue::Option => "synthetic quantity or package option is unsupported",
                    OfferIssue::Period => "synthetic month period is unsupported",
                },
            )
        })
    }

    async fn send_synthetic(
        &self,
        ctx: &RequestContext,
        spec: &Spec<'_>,
        prepared: Prepared,
        state: &mut Run,
    ) -> Result<(), Error> {
        let ids: Vec<_> = prepared.ids.iter().map(String::as_str).collect();
        if ids.iter().any(|id| state.evidence.id_reflects(id)) {
            return Err(quota_invalid(spec.op, &state.evidence));
        }
        if let Some(end) = ctx.check(self.session.clock()) {
            return Err(end.into());
        }
        let request = mutation(spec.op, prepared.path, &Method::POST, OK, true);
        let mut attempt = Attempt::new(
            spec.op,
            Kind::Synthetic,
            prepared.goal,
            std::mem::take(&mut state.evidence),
        );
        let sent = self
            .send_quota(ctx, &request, &[], Some(&prepared.body), &ids, &mut attempt)
            .await;
        match sent {
            Ok(SendState::Refused {
                error,
                dispatch,
                status,
            }) => {
                state.evidence = attempt.evidence;
                state.dispatch = dispatch;
                if dispatch == WriteDispatch::MayHaveBeenSent {
                    state.draft.sent = if status == 0 { "unknown" } else { "yes" };
                    state.draft.stage = "refused";
                    state.draft.status = status;
                }
                return Err(error);
            }
            Ok(SendState::Pending {
                failure: Some(error),
                ..
            }) => {
                state.adopt(&attempt);
                return Err(unsettled(spec.op, Some(error)));
            }
            Ok(SendState::Pending {
                reply: Some(reply), ..
            }) => {
                state.adopt(&attempt);
                let (order, payment) = decode_order(
                    &reply,
                    &request,
                    &attempt.evidence,
                    SYNTHETIC_REDIRECT,
                    true,
                )
                .map_err(|error| unsettled(spec.op, Some(error)))?;
                attempt.order_id = order;
                attempt.payment_pending = payment;
                self.quota_ledger.put(&attempt)?;
                state.adopt(&attempt);
            }
            Ok(SendState::Pending { .. }) => {
                state.adopt(&attempt);
                return Err(unsettled(spec.op, None));
            }
            Err(error) => {
                if let Ok(Some(pending)) = self.quota_ledger.get(Kind::Synthetic) {
                    state.adopt(&pending);
                    return Err(unsettled(spec.op, Some(error)));
                }
                state.evidence = attempt.evidence;
                return Err(error);
            }
        }
        if attempt.payment_pending {
            return Err(unsettled(spec.op, None));
        }
        if spec.no_wait {
            state.draft.stage = "accepted";
            return Ok(());
        }
        match self
            .reconcile_quota(ctx, &mut attempt, spec.op, ORDER_BOUND)
            .await
        {
            Ok(quota) => {
                state.confirm(&attempt, quota);
                Ok(())
            }
            Err(error) => {
                state.adopt(&attempt);
                Err(error)
            }
        }
    }
}

struct Run {
    draft: Draft,
    evidence: Evidence,
    dispatch: WriteDispatch,
    expected_end: Option<OffsetDateTime>,
}
impl Default for Run {
    fn default() -> Self {
        Self {
            draft: Draft::default(),
            evidence: Evidence::new(),
            dispatch: WriteDispatch::NotSent,
            expected_end: None,
        }
    }
}
impl Run {
    fn adopt(&mut self, attempt: &Attempt) {
        self.draft = Draft::retained(attempt);
        self.evidence.absorb(attempt.evidence.credentials());
        self.dispatch = WriteDispatch::MayHaveBeenSent;
    }
    fn confirm(&mut self, attempt: &Attempt, quota: Option<crate::monitor::Quota>) {
        self.adopt(attempt);
        self.draft.stage = "confirmed";
        self.draft.quota = quota;
        self.draft.payment_pending = false;
    }
    fn reset(&mut self) {
        self.draft = Draft::default();
        self.dispatch = WriteDispatch::NotSent;
    }
}
struct Prepared {
    goal: Goal,
    path: &'static str,
    ids: Vec<String>,
    body: Value,
}

pub(super) fn changed(op: &str) -> Error {
    Error::new(ErrorClass::Api)
        .with_outer_detail(crate::monitor::ErrorDetail::QuotaChanged)
        .with_outer_detail(format!("{op}: quota terms changed"))
}

fn free_price(spec: &Spec<'_>, evidence: &Evidence, listed: f64, quoted: f64) -> Result<(), Error> {
    price_guard(spec.op, evidence, listed, quoted, spec.cap)?;
    if listed.max(quoted) != 0.0 {
        return Err(invalid(spec.op, "paid synthetic quota is unsupported"));
    }
    Ok(())
}
fn final_guard<T>(value: Result<T, Error>, op: &str) -> Result<T, Error> {
    value.map_err(|error| {
        if error.has_class(ErrorClass::InvalidInput) {
            changed(op)
        } else {
            error
        }
    })
}
fn quota_invalid(op: &'static str, evidence: &Evidence) -> Error {
    mutation(op, "", &Method::POST, OK, false).error(
        Some(200),
        "InvalidResponse",
        "quota response had invalid structure",
        evidence,
    )
}
fn details<'a>(
    op: &'static str,
    snapshot: &'a Snapshot,
    evidence: &Evidence,
) -> Result<&'a super::state::Details, Error> {
    snapshot
        .details
        .as_ref()
        .ok_or_else(|| quota_invalid(op, evidence))
}
