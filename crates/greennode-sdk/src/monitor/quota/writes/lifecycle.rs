use crate::wire::product::options::PathIds;
use http::{Method, StatusCode};
use serde_json::json;

use crate::monitor::mutation::not_sent;
use crate::monitor::wire::check_path_id;
use crate::monitor::{Client, ErrorDetail, Quota};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::notification::notification_kind;
use super::super::{Screen, output};
use super::state::{
    Attempt, Goal, Kind, Observation, SendState, Snapshot, field, invalid, mutation, price_guard,
    quota_error, result, screen_quota, unsettled, validate_cap, validate_profile,
};

const DELETE_NOTIFY: &str = "monitor.DeleteNotificationQuota";
const DELETE_SYNTH: &str = "monitor.DeleteSyntheticQuota";
const RENEW_NOTIFY: &str = "monitor.SetNotificationQuotaAutoRenew";
const RENEW_SYNTH: &str = "monitor.SetSyntheticQuotaAutoRenew";
const OK: &[StatusCode] = &[StatusCode::OK];
const NO_CONTENT: &[StatusCode] = &[StatusCode::NO_CONTENT];

/// Disables an Email or SMS quota's auto-renew setting.
#[derive(Clone, Debug)]
pub struct SetNotificationQuotaAutoRenewInput {
    /// Email or SMS.
    pub kind: String,
    /// Explicit desired setting. Only false is supported.
    pub enabled: bool,
    /// Maximum quoted price.
    pub max_price: f64,
    /// Optional guard for the current quota ID.
    pub expected_quota_id: String,
}
impl SetNotificationQuotaAutoRenewInput {
    /// Selects a notification quota and its desired setting.
    pub fn new(kind: impl Into<String>, enabled: bool) -> Self {
        Self {
            kind: kind.into(),
            enabled,
            max_price: 0.0,
            expected_quota_id: String::new(),
        }
    }
}

/// Enables auto-renew on an eligible free synthetic quota.
#[derive(Clone, Debug)]
pub struct SetSyntheticQuotaAutoRenewInput {
    /// Explicit desired setting. Only true is supported.
    pub enabled: bool,
    /// Maximum quoted price.
    pub max_price: f64,
    /// Optional guard for the current quota ID.
    pub expected_quota_id: String,
}
impl SetSyntheticQuotaAutoRenewInput {
    /// Supplies the explicit desired setting.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            max_price: 0.0,
            expected_quota_id: String::new(),
        }
    }
}

/// Deletes an active notification quota with auto-renew and POC off.
#[derive(Clone, Debug)]
pub struct DeleteNotificationQuotaInput {
    /// Email or SMS.
    pub kind: String,
    /// Requests purge, which is unsupported.
    pub purge: bool,
    /// Optional guard for the current quota ID.
    pub expected_quota_id: String,
}
impl DeleteNotificationQuotaInput {
    /// Selects the notification quota kind to delete.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            purge: false,
            expected_quota_id: String::new(),
        }
    }
}

/// Deletes a synthetic quota and its associated checks.
#[derive(Clone, Debug)]
pub struct DeleteSyntheticQuotaInput {
    /// Required guard for the current quota ID.
    pub expected_quota_id: String,
    /// Requests purge, which is unsupported.
    pub purge: bool,
}
impl DeleteSyntheticQuotaInput {
    /// Supplies the required quota identity guard.
    pub fn new(expected_quota_id: impl Into<String>) -> Self {
        Self {
            expected_quota_id: expected_quota_id.into(),
            purge: false,
        }
    }
}

output! {
    /// Frozen quota and outcome of changing auto-renew.
    struct SetQuotaAutoRenewOutput {
        /// Intended quota snapshot.
        quota:Option<Quota> =>"Quota",
        /// Latest stage reached by the attempt.
        stage:String=>"Stage",
        /// Whether a read confirmed the requested setting.
        changed:bool=>"Changed",
    }
}
output! {
    /// Frozen quota and outcome of deleting it.
    struct DeleteQuotaOutput {
        /// Intended quota snapshot.
        quota:Option<Quota> =>"Quota",
        /// Latest stage reached by the attempt.
        stage:String=>"Stage",
        /// Whether the quota was purged.
        purged:bool=>"Purged",
    }
}
/// Outcome of setting notification auto-renew.
pub type SetNotificationQuotaAutoRenewOutput = SetQuotaAutoRenewOutput;
/// Outcome of setting synthetic auto-renew.
pub type SetSyntheticQuotaAutoRenewOutput = SetQuotaAutoRenewOutput;
/// Outcome of deleting a notification quota.
pub type DeleteNotificationQuotaOutput = DeleteQuotaOutput;
/// Outcome of deleting a synthetic quota.
pub type DeleteSyntheticQuotaOutput = DeleteQuotaOutput;

struct Lifecycle<'a> {
    op: &'static str,
    kind: Kind,
    expected: &'a str,
    purge: bool,
    action: LifecycleAction,
    cap: f64,
}
#[derive(Clone, Copy)]
enum LifecycleAction {
    Delete,
    Renew(bool),
}
#[derive(Default)]
struct LifecycleResult {
    quota: Option<Quota>,
    stage: String,
    changed: bool,
}
impl LifecycleResult {
    fn prepared(quota: Quota) -> Self {
        Self {
            quota: Some(quota),
            stage: "prepared".into(),
            changed: false,
        }
    }
    fn from_goal(goal: &Goal) -> Self {
        let quota = match goal {
            Goal::Delete { before } | Goal::RenewSetting { before, .. } => {
                Some(before.quota.clone())
            }
            _ => None,
        };
        Self {
            quota,
            stage: "sent".into(),
            changed: false,
        }
    }
    fn auto_output(self) -> SetQuotaAutoRenewOutput {
        SetQuotaAutoRenewOutput {
            quota: self.quota,
            stage: self.stage,
            changed: self.changed,
        }
    }
    fn delete_output(self) -> DeleteQuotaOutput {
        DeleteQuotaOutput {
            quota: self.quota,
            stage: self.stage,
            purged: false,
        }
    }
}

impl Client {
    /// Disables auto-renew for an Email or SMS quota and confirms the new setting.
    pub async fn set_notification_quota_auto_renew(
        &self,
        ctx: &RequestContext,
        input: SetNotificationQuotaAutoRenewInput,
    ) -> Result<SetNotificationQuotaAutoRenewOutput, Failure<SetNotificationQuotaAutoRenewOutput>>
    {
        let kind = Kind::from(notification_kind(RENEW_NOTIFY, &input.kind).map_err(not_sent)?);
        self.quota_lifecycle(
            ctx,
            Lifecycle {
                op: RENEW_NOTIFY,
                kind,
                expected: &input.expected_quota_id,
                purge: false,
                action: LifecycleAction::Renew(input.enabled),
                cap: input.max_price,
            },
            LifecycleResult::auto_output,
        )
        .await
    }
    /// Enables auto-renew for an eligible free synthetic quota and confirms the new setting.
    pub async fn set_synthetic_quota_auto_renew(
        &self,
        ctx: &RequestContext,
        input: SetSyntheticQuotaAutoRenewInput,
    ) -> Result<SetSyntheticQuotaAutoRenewOutput, Failure<SetSyntheticQuotaAutoRenewOutput>> {
        self.quota_lifecycle(
            ctx,
            Lifecycle {
                op: RENEW_SYNTH,
                kind: Kind::Synthetic,
                expected: &input.expected_quota_id,
                purge: false,
                action: LifecycleAction::Renew(input.enabled),
                cap: input.max_price,
            },
            LifecycleResult::auto_output,
        )
        .await
    }
    /// Deletes an active notification quota only after verifying its frozen terms.
    pub async fn delete_notification_quota(
        &self,
        ctx: &RequestContext,
        input: DeleteNotificationQuotaInput,
    ) -> Result<DeleteNotificationQuotaOutput, Failure<DeleteNotificationQuotaOutput>> {
        let kind = Kind::from(notification_kind(DELETE_NOTIFY, &input.kind).map_err(not_sent)?);
        self.quota_lifecycle(
            ctx,
            Lifecycle {
                op: DELETE_NOTIFY,
                kind,
                expected: &input.expected_quota_id,
                purge: input.purge,
                action: LifecycleAction::Delete,
                cap: 0.0,
            },
            LifecycleResult::delete_output,
        )
        .await
    }
    /// Deletes a synthetic quota after matching its required identity guard.
    pub async fn delete_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: DeleteSyntheticQuotaInput,
    ) -> Result<DeleteSyntheticQuotaOutput, Failure<DeleteSyntheticQuotaOutput>> {
        self.quota_lifecycle(
            ctx,
            Lifecycle {
                op: DELETE_SYNTH,
                kind: Kind::Synthetic,
                expected: &input.expected_quota_id,
                purge: input.purge,
                action: LifecycleAction::Delete,
                cap: 0.0,
            },
            LifecycleResult::delete_output,
        )
        .await
    }

    async fn quota_lifecycle<T: Screen>(
        &self,
        ctx: &RequestContext,
        input: Lifecycle<'_>,
        project: fn(LifecycleResult) -> T,
    ) -> Result<T, Failure<T>> {
        let mut evidence = Evidence::new();
        let initial = |error: Error| {
            if input.op == DELETE_SYNTH
                && let Ok(Some(attempt)) = self.quota_ledger.get(input.kind)
            {
                let error = if !input.expected.is_empty()
                    && PathIds::Strict.allows(input.expected)
                    && input.expected != attempt.goal.id()
                {
                    error.with_cause(changed_intent(input.op))
                } else {
                    error
                };
                return lifecycle_result(
                    project,
                    LifecycleResult {
                        stage: "sent".into(),
                        ..Default::default()
                    },
                    Err(unsettled(input.op, Some(error))),
                    WriteDispatch::MayHaveBeenSent,
                    &attempt.evidence,
                );
            }
            lifecycle_not_sent(input.op, project, error, &Evidence::new())
        };
        if let Err(error) = validate_profile(self, input.op) {
            return initial(error);
        }
        if input.op == DELETE_SYNTH && input.expected.is_empty() {
            return initial(invalid(input.op, "ExpectedQuotaID is required"));
        }
        if !input.expected.is_empty()
            && let Err(failure) = check_path_id::<T>(input.op, "ExpectedQuotaID", input.expected)
        {
            return initial(failure.into_parts().0);
        }
        if let LifecycleAction::Renew(enabled) = input.action {
            if let Err(error) = validate_cap(input.op, input.cap) {
                return initial(error);
            }
            if enabled != (input.kind == Kind::Synthetic) {
                let action = if enabled { "enable" } else { "disable" };
                return initial(invalid(
                    input.op,
                    &format!("{} auto-renew {action} is unsupported", input.kind.name()),
                ));
            }
        }
        if input.purge {
            return initial(invalid(
                input.op,
                &format!("{} quota purge is unsupported", input.kind.name()),
            ));
        }
        let _gate = match self.quota_ledger.acquire(ctx, self.session.clock()).await {
            Ok(guard) => guard,
            Err(error) => return initial(error),
        };
        if let Some(mut attempt) = self.quota_ledger.get(input.kind).map_err(not_sent)? {
            let mismatch = !input.expected.is_empty() && input.expected != attempt.goal.id();
            let matched = attempt.op == input.op && !mismatch;
            let was_order = matches!(
                attempt.goal,
                Goal::SyntheticCreate { .. } | Goal::SyntheticChange { .. }
            );
            if mismatch && input.op != DELETE_SYNTH && !was_order {
                return lifecycle_not_sent(
                    input.op,
                    project,
                    invalid(input.op, "ExpectedQuotaID differs from intended quota"),
                    &attempt.evidence,
                );
            }
            let mut out = LifecycleResult::from_goal(&attempt.goal);
            let bound = attempt.goal.bound();
            match self
                .reconcile_quota(ctx, &mut attempt, input.op, bound)
                .await
            {
                Ok(quota) if matched => {
                    out.quota = quota;
                    out.stage = "settled".into();
                    out.changed = matches!(input.action, LifecycleAction::Renew(_));
                    return lifecycle_result(
                        project,
                        out,
                        Ok(()),
                        WriteDispatch::MayHaveBeenSent,
                        &attempt.evidence,
                    );
                }
                Ok(_) => {
                    let error = if mismatch && was_order && input.op != DELETE_SYNTH {
                        super::synthetic::changed(input.op)
                    } else {
                        changed_intent(input.op)
                    };
                    return lifecycle_not_sent(input.op, project, error, &attempt.evidence);
                }
                Err(error) if matched || input.op == DELETE_SYNTH => {
                    if !attempt.evidence.can_retain_partial() {
                        out.quota = None;
                    }
                    let error = if mismatch && input.op == DELETE_SYNTH {
                        error.with_cause(changed_intent(input.op))
                    } else {
                        error
                    };
                    return lifecycle_result(
                        project,
                        out,
                        Err(error),
                        WriteDispatch::MayHaveBeenSent,
                        &attempt.evidence,
                    );
                }
                Err(error) => {
                    let error = if mismatch && was_order {
                        error.with_cause(super::synthetic::changed(input.op))
                    } else {
                        error
                    };
                    return Err(Failure::new(
                        quota_error(error, &attempt.evidence),
                        None,
                        WriteDispatch::MayHaveBeenSent,
                    ));
                }
            }
        }
        let initial_evidence = match self.write_evidence(ctx, &[input.expected]) {
            Ok(evidence) => evidence,
            Err(error) => return initial(error),
        };
        evidence.absorb(initial_evidence.credentials());
        let before = match self
            .observe_quota(ctx, input.kind, input.op, &mut evidence, true, false)
            .await
            .and_then(Observation::present)
        {
            Ok(value) => value,
            Err(error) => return initial(error),
        };
        if !input.expected.is_empty() && input.expected != before.id() {
            return initial(invalid(
                input.op,
                "ExpectedQuotaID differs from intended quota",
            ));
        }
        if matches!(input.action, LifecycleAction::Delete) {
            if let Err(error) = delete_eligible(input.op, &before) {
                return initial(error);
            }
            if input.kind == Kind::Synthetic
                && before
                    .end()
                    .is_none_or(|end| end <= self.session.clock().utc_now())
            {
                return initial(invalid(input.op, "delete requires unexpired quota"));
            }
        } else if let LifecycleAction::Renew(enabled) = input.action {
            if field(before.quota.auto_renew) == Some(enabled) {
                if enabled && field(before.quota.auto_renew_period) != Some(1) {
                    return initial(invalid(input.op, "enabled period is not one month"));
                }
                return lifecycle_result(
                    project,
                    LifecycleResult::prepared(before.quota),
                    Ok(()),
                    WriteDispatch::NotSent,
                    &evidence,
                );
            }
            if enabled {
                let Some(details) = before
                    .details
                    .as_ref()
                    .filter(|details| details.api > 0 && details.price.is_some())
                else {
                    return initial(
                        mutation(input.op, "", &Method::POST, OK, false).invalid(200, &evidence),
                    );
                };
                if details.price != Some(0.0) {
                    return initial(invalid(
                        input.op,
                        "paid synthetic auto-renew enable is unsupported",
                    ));
                }
                let body = json!({"redirectUrl":"","packageId":before.package(),"quantity":details.api,"resourceId":before.id()});
                let quote = match self
                    .quota_offer_price(
                        ctx,
                        input.op,
                        input.kind,
                        &[before.id(), before.package(), &details.class_id],
                        &body,
                        &mut evidence,
                    )
                    .await
                {
                    Ok(quote) => quote,
                    Err(error) => return initial(error),
                };
                if let Err(error) = price_guard(input.op, &evidence, 0.0, quote, input.cap) {
                    return initial(error);
                }
            }
        }
        let mut out = LifecycleResult::prepared(before.quota.clone());
        let refreshed = self
            .observe_quota(ctx, input.kind, input.op, &mut evidence, true, false)
            .await
            .and_then(Observation::present);
        let fresh = match refreshed {
            Ok(fresh) => fresh,
            Err(error) => return prepared_error(&input, project, out, error, &evidence),
        };
        if matches!(input.action, LifecycleAction::Delete)
            && let Err(error) = delete_eligible(input.op, &fresh)
        {
            return prepared_error(&input, project, out, error, &evidence);
        }
        if !before.same(&fresh)
            || (input.op == DELETE_SYNTH
                && fresh
                    .end()
                    .is_none_or(|end| end <= self.session.clock().utc_now()))
        {
            return prepared_error(
                &input,
                project,
                out,
                invalid(input.op, "quota terms changed before send"),
                &evidence,
            );
        }
        let id = before.id().to_owned();
        let (request, body, goal) = match input.action {
            LifecycleAction::Delete => (
                mutation(
                    input.op,
                    input.kind.current(),
                    &Method::DELETE,
                    NO_CONTENT,
                    input.op == DELETE_SYNTH,
                ),
                None,
                Goal::Delete { before },
            ),
            LifecycleAction::Renew(enabled) => (
                mutation(input.op, input.kind.renewal(), &Method::POST, OK, false),
                Some(json!({"resourceId":id,"enabled":enabled,"period":1})),
                Goal::RenewSetting { before, enabled },
            ),
        };
        let mut attempt = Attempt::new(input.op, input.kind, goal, evidence);
        let parts = if body.is_some() {
            std::slice::from_ref(&id)
        } else {
            &[]
        };
        let path: Vec<&str> = parts.iter().map(String::as_str).collect();
        let mut ids = vec![id.as_str()];
        if let Goal::Delete { before } | Goal::RenewSetting { before, .. } = &attempt.goal {
            ids.push(before.package());
            if let Some(details) = before.details.as_ref() {
                ids.push(&details.class_id);
            }
        }
        let owned_ids: Vec<String> = ids.into_iter().map(str::to_owned).collect();
        let ids: Vec<&str> = owned_ids.iter().map(String::as_str).collect();
        let send = self
            .send_quota(ctx, &request, &path, body.as_ref(), &ids, &mut attempt)
            .await
            .map_err(not_sent)?;
        match send {
            SendState::Refused {
                error, dispatch, ..
            } => {
                if input.op == DELETE_SYNTH {
                    return lifecycle_result(project, out, Err(error), dispatch, &attempt.evidence);
                }
                return Err(Failure::new(error, None, dispatch));
            }
            SendState::Pending { failure, reply } => {
                out.stage = "sent".into();
                if let Some(error) = failure {
                    return lifecycle_result(
                        project,
                        out,
                        Err(unsettled(input.op, Some(error))),
                        WriteDispatch::MayHaveBeenSent,
                        &attempt.evidence,
                    );
                }
                if reply.is_none_or(|reply| !reply.is_empty()) {
                    return lifecycle_result(
                        project,
                        out,
                        Err(unsettled(
                            input.op,
                            Some(request.invalid(attempt.observed_status, &attempt.evidence)),
                        )),
                        WriteDispatch::MayHaveBeenSent,
                        &attempt.evidence,
                    );
                }
            }
        }
        let bound = attempt.goal.bound();
        match self
            .reconcile_quota(ctx, &mut attempt, input.op, bound)
            .await
        {
            Ok(quota) => {
                out.quota = quota;
                out.stage = "settled".into();
                out.changed = matches!(input.action, LifecycleAction::Renew(_));
                lifecycle_result(
                    project,
                    out,
                    Ok(()),
                    WriteDispatch::MayHaveBeenSent,
                    &attempt.evidence,
                )
            }
            Err(error) => {
                if !attempt.evidence.can_retain_partial() {
                    out.quota = None;
                }
                lifecycle_result(
                    project,
                    out,
                    Err(error),
                    WriteDispatch::MayHaveBeenSent,
                    &attempt.evidence,
                )
            }
        }
    }
}

fn delete_eligible(op: &str, snapshot: &Snapshot) -> Result<(), Error> {
    match snapshot.status() {
        "ACTIVE" => (),
        "CREATING" | "IN-TRASH" | "ERROR" => return Err(invalid(op, "delete requires ACTIVE")),
        _ => return Err(unexpected(op)),
    }
    if field(snapshot.quota.auto_renew) != Some(false) || field(snapshot.quota.poc) != Some(false) {
        return Err(invalid(op, "delete requires auto-renew and POC off"));
    }
    Ok(())
}
pub(super) fn unexpected(op: &str) -> Error {
    Error::new(ErrorClass::Api)
        .with_outer_detail(ErrorDetail::UnexpectedStatus)
        .with_outer_detail(format!(
            "{op}: unknown quota status: monitor: unexpected check status"
        ))
}
pub(super) fn changed_intent(op: &str) -> Error {
    invalid(op, "prior attempt changed intent")
}
fn prepared_error<T: Screen>(
    input: &Lifecycle<'_>,
    project: fn(LifecycleResult) -> T,
    out: LifecycleResult,
    error: Error,
    evidence: &Evidence,
) -> Result<T, Failure<T>> {
    if input.op == DELETE_SYNTH {
        return lifecycle_result(project, out, Err(error), WriteDispatch::NotSent, evidence);
    }
    Err(not_sent(error))
}
fn lifecycle_not_sent<T: Screen>(
    op: &'static str,
    project: fn(LifecycleResult) -> T,
    error: Error,
    evidence: &Evidence,
) -> Result<T, Failure<T>> {
    if op == DELETE_SYNTH {
        lifecycle_result(
            project,
            LifecycleResult::default(),
            Err(error),
            WriteDispatch::NotSent,
            evidence,
        )
    } else {
        Err(not_sent(quota_error(error, evidence)))
    }
}

fn lifecycle_result<T: Screen>(
    project: fn(LifecycleResult) -> T,
    mut out: LifecycleResult,
    outcome: Result<(), Error>,
    dispatch: WriteDispatch,
    evidence: &Evidence,
) -> Result<T, Failure<T>> {
    screen_quota(&mut out.quota, evidence);
    result(project(out), outcome, dispatch, evidence)
}
