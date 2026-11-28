use std::collections::{BTreeMap, HashMap};

use serde_json::value::RawValue;

use super::principal_guard::{guard_read, optional_field, require_safe};
use super::wire::{ObservedResponse, Wire};
use super::writes::{confirmed_identity, error_reason, guard_failed, guard_refusal, not_sent};
use super::{Api, Client, LIST_CAP, OBJECT_CAP, Operation, UserDetail, WriteError, read_operation};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RecoveryAdvice, RequestContext};

pub(super) const USER: &str = "accounts-api/v1/iam-users/{}";
const USER_POLICIES: &str = "policies-api/v1/user-attachments/iam-users/{}/policies";
const USER_GROUPS: &str = "policies-api/v1/user-attachments/iam-users/{}/groups";

#[derive(Default)]
struct UserGroup {
    id: String,
    policies: Option<Vec<String>>,
}

impl Wire for UserGroup {
    const SHAPE: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "id",
            shape: Shape::Any,
        },
        ShapeField {
            name: "policies",
            shape: Vec::<String>::SHAPE,
        },
    ]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields: BTreeMap<String, &RawValue> =
            serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        Ok(Self {
            id: optional_field(&fields, "id")?.unwrap_or_default(),
            policies: optional_field(&fields, "policies")?,
        })
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    pub(super) async fn fetch_verified_user<O>(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<UserDetail, Failure<O>> {
        let operation = Operation {
            sensitive: true,
            reflection: crate::wire::product::operation::Reflection::Refuse,
            ..read_operation(op, USER, OBJECT_CAP, false)
        };
        let user: Option<UserDetail> = self
            .fetch(
                ctx,
                Api::Accounts,
                &operation,
                &[("UserID", id)],
                &[],
                evidence,
            )
            .await?;
        let invalid = || {
            guard_refusal(
                &format!("iam: {op}: target identity or origin is unproven"),
                None,
            )
        };
        let user = user.ok_or_else(invalid)?;
        confirmed_identity::<O>(&user.id, id, "user identity is unproven", evidence)
            .map_err(|_| invalid())?;
        if user.from != "iam" || evidence.id_reflects(&user.id) || evidence.id_reflects(&user.from)
        {
            return Err(invalid());
        }
        require_safe(&user, "IAM user response withheld", evidence)?;
        Ok(user)
    }

    pub(super) async fn guard_user<O>(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        id: &str,
        administer: bool,
        evidence: &mut Evidence,
    ) -> Result<UserDetail, Failure<O>> {
        let caller = self.require_classified_caller(ctx, op, evidence).await?;
        if caller.user_type == "iam-user" {
            if !PathIds::Strict.allows(&caller.user_id) || evidence.id_reflects(&caller.user_id) {
                return Err(super::writes::proved_refusal(
                    evidence,
                    super::writes::ProofRefusal::Privileged,
                    &format!(
                        "iam: refused: target is protected: {op}: caller identity is unusable"
                    ),
                ));
            }
            if caller.user_id.eq_ignore_ascii_case(id) {
                return Err(guard_refusal(
                    &format!("iam: refused: target is the caller: {op}"),
                    Some(WriteError::SelfChange),
                ));
            }
        }
        let target = self
            .fetch_verified_user(ctx, op, id, evidence)
            .await
            .map_err(|failure: Failure<O>| {
                if failure.error().detail::<ObservedResponse>().is_some()
                    || failure
                        .error()
                        .api_error()
                        .and_then(crate::ApiError::observed_status)
                        .is_some()
                {
                    guard_failed(failure)
                } else {
                    unproven_origin(op, Some(failure.into_parts().0), evidence)
                }
            })?;
        if !administer {
            return Ok(target);
        }
        let actions = self.guard_write_actions(ctx, true, evidence).await?;
        let direct = self
            .paged_policy_ids(ctx, op, USER_POLICIES, ("UserID", id), evidence)
            .await?;
        let mut seen = HashMap::new();
        let protected = |evidence: &Evidence| {
            super::writes::proved_refusal(
                evidence,
                super::writes::ProofRefusal::Privileged,
                &format!("{op}: iam: refused: target is protected"),
            )
        };
        if self
            .policy_ids_are_privileged(ctx, direct, &actions, &mut seen, evidence)
            .await?
        {
            return Err(protected(evidence));
        }
        let groups: Option<Vec<UserGroup>> = self
            .fetch(
                ctx,
                Api::Policies,
                &guard_read(op, USER_GROUPS, LIST_CAP),
                &[("UserID", id)],
                &[],
                evidence,
            )
            .await
            .map_err(guard_failed)?;
        let groups = groups.ok_or_else(|| {
            guard_refusal(
                &format!("iam: guard: {op}: attachment list was null or missing"),
                None,
            )
        })?;
        for group in groups {
            if evidence.id_reflects(&group.id) {
                return Err(guard_refusal("IAM user group identity withheld", None));
            }
            let policies = group.policies.ok_or_else(|| {
                guard_refusal(
                    &format!("iam: guard: {op}: a user's group had no policies field"),
                    None,
                )
            })?;
            if self
                .policy_ids_are_privileged(ctx, policies, &actions, &mut seen, evidence)
                .await?
            {
                return Err(protected(evidence));
            }
        }
        if evidence.id_reflects(id) || evidence.id_reflects(&target.id) {
            return Err(unproven_origin(op, None, evidence));
        }
        require_safe(&target, "IAM user response withheld", evidence)?;
        Ok(target)
    }
}

fn unproven_origin<O>(op: &str, cause: Option<Error>, evidence: &Evidence) -> Failure<O> {
    const REFUSAL: &str = "iam: refused: target IAM origin is unproven";
    let message = format!("{REFUSAL}: {op}: target identity or origin is unproven");
    let (error, message) = match cause {
        Some(cause) => {
            let message = format!(
                "{op}: read target user: {REFUSAL}: {}",
                error_reason(&cause)
            );
            (Error::new(ErrorClass::Api).with_cause(cause), message)
        }
        None => (Error::new(ErrorClass::Api), message),
    };
    let message = if evidence.can_retain_partial() && !evidence.text_reflects(&message) {
        &message
    } else {
        REFUSAL
    };
    not_sent(
        error
            .with_outer_detail(WriteError::OriginNotIam)
            .with_outer_detail(RecoveryAdvice::new(message)),
    )
}
