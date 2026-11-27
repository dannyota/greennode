use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex_lite::Regex;
use serde::Serialize;
use serde_json::value::RawValue;

use super::wire::{self, Wire};
use super::writes::{
    check_cached_actions, confirmed_identity, guard_failed, guard_refusal, keep_actions_check,
};
use super::{
    Action, Api, Client, LIST_CAP, OBJECT_CAP, Operation, Policy, PolicySummary, WriteError,
    read_operation,
};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Failure, RequestContext};

const ACCOUNT_POLICIES: &str = "policies-api/v1/user-attachments/service-accounts/{}/policies";
const ACTIONS: &str = "policies-api/v1/actions";
const POLICY: &str = "policies-api/v1/policies/{}";

#[derive(Default)]
struct AttachmentPage {
    items: Option<Vec<PolicySummary>>,
    total_items: Option<i64>,
    total_pages: Option<i64>,
}

impl Wire for AttachmentPage {
    const SHAPE: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "data",
            shape: Vec::<PolicySummary>::SHAPE,
        },
        ShapeField {
            name: "totalItems",
            shape: Shape::Any,
        },
        ShapeField {
            name: "totalPages",
            shape: Shape::Any,
        },
    ]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields: BTreeMap<String, &RawValue> =
            serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        Ok(Self {
            items: optional_field(&fields, "data")?,
            total_items: optional_field(&fields, "totalItems")?,
            total_pages: optional_field(&fields, "totalPages")?,
        })
    }

    fn scan_operands(
        &self,
        scan: &crate::wire::product::output::FieldScan<'_>,
        identifier: bool,
    ) -> bool {
        self.items.scan_operands(scan, identifier)
    }
}

pub(super) fn optional_field<T: Wire>(
    fields: &BTreeMap<String, &RawValue>,
    key: &str,
) -> Result<Option<T>, serde_json::Error> {
    wire::field(fields, key)
        .map(Option::<T>::decode)
        .transpose()
        .map(Option::flatten)
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    pub(super) async fn guard_service_account<O>(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Failure<O>> {
        let caller = self.require_classified_caller(ctx, op, evidence).await?;
        if matches!(caller.user_type.as_str(), "user-sa" | "service-sa") {
            return Err(guard_refusal(
                &format!(
                    "iam: refused: target is the caller: {op}: the caller is a service account"
                ),
                Some(WriteError::SelfChange),
            ));
        }
        let actions = self.guard_write_actions(ctx, true, evidence).await?;
        let ids = self
            .paged_policy_ids(
                ctx,
                op,
                ACCOUNT_POLICIES,
                ("ServiceAccountID", id),
                evidence,
            )
            .await?;
        let protected = self
            .policy_ids_are_privileged(ctx, ids, &actions, &mut HashMap::new(), evidence)
            .await?;
        if protected {
            return Err(super::writes::proved_refusal(
                evidence,
                super::writes::ProofRefusal::Privileged,
                &format!(
                    "iam: refused: target is protected: {op}: the service account has a privileged policy attached"
                ),
            ));
        }
        if evidence.id_reflects(id) {
            return Err(guard_failed(super::invalid_input(
                "IAM request operand withheld".into(),
            )));
        }
        Ok(())
    }

    pub(super) async fn guard_write_actions<O>(
        &self,
        ctx: &RequestContext,
        sensitive: bool,
        evidence: &mut Evidence,
    ) -> Result<Vec<String>, Failure<O>> {
        let unsafe_text = if sensitive {
            "iam: iam.ListActions: unsafe action list"
        } else {
            "iam: iam.ListActions: unsafe guard response"
        };
        let mut slot = self.lock_bounded(&self.guard.write_actions, ctx).await?;
        if !slot.is_empty() {
            check_cached_actions(&slot, evidence)?;
            keep_actions_check(&slot, evidence);
            return Ok(slot.clone());
        }
        let actions: Vec<Action> = self
            .fetch(
                ctx,
                Api::Policies,
                &Operation {
                    sensitive,
                    ..read_operation("iam.ListActions", ACTIONS, LIST_CAP, false)
                },
                &[],
                &[("product", "iam")],
                evidence,
            )
            .await
            .map_err(guard_failed)?;
        require_safe(&actions, unsafe_text, evidence)?;
        let names = write_action_names(&actions)
            .filter(|names| !names.is_empty())
            .ok_or_else(|| {
                guard_refusal(
                    "iam: guard: refused: the account's IAM action list names no write action",
                    None,
                )
            })?;
        let derived_text = if sensitive {
            "iam: iam.ListActions: unsafe cached guard response"
        } else {
            unsafe_text
        };
        require_safe(&names, derived_text, evidence)?;
        keep_actions_check(&names, evidence);
        slot.clone_from(&names);
        Ok(names)
    }

    pub(super) async fn paged_policy_ids<O>(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        route: &'static str,
        id: (&str, &str),
        evidence: &mut Evidence,
    ) -> Result<Vec<String>, Failure<O>> {
        let page: AttachmentPage = self
            .fetch(
                ctx,
                Api::Policies,
                &guard_read(op, route, LIST_CAP),
                &[id],
                &[("pageNumber", "0"), ("pageSize", "10000")],
                evidence,
            )
            .await
            .map_err(guard_failed)?;
        let refuse = |reason| guard_refusal(&format!("iam: guard: {op}: {reason}"), None);
        let items = page
            .items
            .ok_or_else(|| refuse("attachment list response had no data field"))?;
        let total = page
            .total_items
            .ok_or_else(|| refuse("attachment list response had no totalItems field"))?;
        if usize::try_from(total) != Ok(items.len()) {
            return Err(refuse(
                "attachment list item count does not match totalItems",
            ));
        }
        if page.total_pages.is_some_and(|pages| pages > 1) {
            return Err(refuse("attachment list reports more than one page"));
        }
        require_safe(
            &(total, page.total_pages, &items),
            "iam: guard: policy attachments withheld",
            evidence,
        )?;
        Ok(items.into_iter().map(|item| item.id).collect())
    }

    pub(super) async fn policy_ids_are_privileged<O>(
        &self,
        ctx: &RequestContext,
        ids: Vec<String>,
        actions: &[String],
        seen: &mut HashMap<String, Policy>,
        evidence: &mut Evidence,
    ) -> Result<bool, Failure<O>> {
        for id in ids {
            if evidence.id_reflects(&id) {
                return Err(guard_refusal("IAM policy identity withheld", None));
            }
            let policy = match seen.entry(id) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let policy = self.fetch_guard_policy(ctx, entry.key(), evidence).await?;
                    entry.insert(policy)
                }
            };
            require_safe(policy, "iam: iam.GetPolicy: unsafe policy", evidence)?;
            if policy_has_write_actions(policy, actions) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn fetch_guard_policy<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Policy, Failure<O>> {
        let policy: Policy = self
            .fetch(
                ctx,
                Api::Policies,
                &guard_read("iam.GetPolicy", POLICY, OBJECT_CAP),
                &[("PolicyID", id)],
                &[],
                evidence,
            )
            .await
            .map_err(guard_failed)?;
        confirmed_identity(
            &policy.id,
            id,
            "iam.GetPolicy: policy identity is unproven",
            evidence,
        )
        .map_err(guard_failed)?;
        Ok(policy)
    }
}

fn policy_has_write_actions(policy: &Policy, actions: &[String]) -> bool {
    policy.statements.is_empty()
        || policy.statements.iter().any(|statement| {
            let effect = &statement.effect;
            !effect.eq_ignore_ascii_case("deny")
                && (!effect.eq_ignore_ascii_case("allow")
                    || statement.actions.is_empty()
                    || statement
                        .actions
                        .iter()
                        .any(|pattern| matches_write_action(pattern, actions)))
        })
}

pub(super) fn guard_read(id: &'static str, route: &'static str, cap: usize) -> Operation {
    Operation {
        sensitive: true,
        ..read_operation(id, route, cap, false)
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn require_safe<O>(
    value: &impl Serialize,
    message: &str,
    evidence: &Evidence,
) -> Result<(), Failure<O>> {
    if serde_json::to_value(value).is_ok_and(|value| !evidence.output_reflects(&value)) {
        Ok(())
    } else {
        Err(guard_refusal(message, None))
    }
}

fn write_action_names(actions: &[Action]) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for action in actions {
        let name = action.action.to_ascii_lowercase();
        let name = name.strip_prefix("iam:").unwrap_or(&name);
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return None;
        }
        if !matches!(action.label.as_str(), "List" | "Read" | "Tagging") {
            names.push(format!("iam:{name}"));
        }
    }
    Some(names)
}

fn matches_write_action(pattern: &str, actions: &[String]) -> bool {
    static VALID: LazyLock<Result<Regex, regex_lite::Error>> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9*]+(-[A-Za-z0-9*]+)*(:[A-Za-z0-9*]+)?$"));
    if VALID
        .as_ref()
        .map_or(true, |regex| !regex.is_match(pattern))
    {
        return true;
    }
    let pattern = pattern.to_ascii_lowercase();
    if matches!(pattern.as_str(), "*" | "*:*" | "iam:*") {
        return true;
    }
    let pattern = format!("^{}$", pattern.replace('*', ".*"));
    Regex::new(&pattern).map_or(true, |regex| {
        actions.iter().any(|action| regex.is_match(action))
    })
}
