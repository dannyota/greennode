use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex_lite::Regex;
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use super::groups::GroupAttachments;
use super::wire::{self, Screen, Wire};
use super::writes::{WriteError, confirmed_identity, guard_failed, guard_refusal};
use super::{
    Action, Api, Client, GetCallerIdentityOutput, GroupSummary, LIST_CAP, OBJECT_CAP, Policy,
    PolicySummary, Statement, read_operation,
};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Failure, RequestContext};

pub(super) const POLICY: &str = "policies-api/v1/policies/{}";
const ACTIONS: &str = "iam.ListActions";
const GET_POLICY: &str = "iam.GetPolicy";

pub(super) struct Guard<'a> {
    client: &'a Client,
    ctx: &'a RequestContext,
    op: &'static str,
    evidence: &'a mut Evidence,
    caller: GetCallerIdentityOutput,
    actions: Vec<String>,
    policies: BTreeMap<String, Policy>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Protection {
    None,
    Privileged,
    Caller,
}

pub(super) struct Attachments {
    groups: Vec<GroupSummary>,
    users: Vec<String>,
    accounts: Vec<String>,
}

impl Attachments {
    pub(super) fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.users.is_empty() && self.accounts.is_empty()
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl<'a> Guard<'a> {
    pub(super) async fn new<O>(
        client: &'a Client,
        ctx: &'a RequestContext,
        op: &'static str,
        evidence: &'a mut Evidence,
    ) -> Result<Self, Failure<O>> {
        let caller = client.require_classified_caller(ctx, op, evidence).await?;
        if caller.user_type == "iam-user" && !PathIds::WithUnderscore.allows(&caller.user_id) {
            return Err(guard_refusal(
                "iam: iam.GetCallerIdentity: unsafe caller identity",
                None,
            ));
        }
        Ok(Self {
            client,
            ctx,
            op,
            evidence,
            caller,
            actions: Vec::new(),
            policies: BTreeMap::new(),
        })
    }

    pub(super) async fn load_actions<O>(&mut self) -> Result<(), Failure<O>> {
        let catalog: Vec<Action> = self
            .client
            .fetch(
                self.ctx,
                Api::Policies,
                &read_operation(ACTIONS, "policies-api/v1/actions", LIST_CAP, false),
                &[],
                &[("product", "iam")],
                self.evidence,
            )
            .await
            .map_err(guard_failed)?;
        self.check_response(ACTIONS, &catalog)?;
        for action in catalog {
            let name = action.action.to_ascii_lowercase();
            let action_name = name.strip_prefix("iam:").unwrap_or(&name);
            if action_name.is_empty() || !action_name.bytes().all(|b| b.is_ascii_alphanumeric()) {
                self.actions.clear();
                break;
            }
            if !matches!(action.label.as_str(), "List" | "Read" | "Tagging") {
                self.actions.push(format!("iam:{action_name}"));
            }
        }
        if self.actions.is_empty() {
            return Err(guard_refusal(
                "iam: guard: refused: the account's IAM action list names no write action",
                None,
            ));
        }
        self.check_response(ACTIONS, &self.actions)
    }

    pub(super) async fn policy<O>(&mut self, id: &str) -> Result<Policy, Failure<O>> {
        Ok(self.checked_policy(id).await?.policy)
    }

    pub(super) async fn replacement_policy<O>(
        &mut self,
        id: &str,
    ) -> Result<(Policy, Value), Failure<O>> {
        let snapshot = self.checked_policy(id).await?;
        let statements = snapshot
            .statements
            .as_deref()
            .filter(|_| snapshot.complete)
            .and_then(preservable_statements)
            .ok_or_else(|| {
                guard_refusal(
                    "iam.GetPolicy: incomplete policy replacement snapshot",
                    None,
                )
            })?;
        self.check_response(GET_POLICY, &statements)?;
        Ok((snapshot.policy, statements))
    }

    async fn checked_policy<O>(&mut self, id: &str) -> Result<PolicySnapshot, Failure<O>> {
        let snapshot = self
            .client
            .policy_snapshot(self.ctx, id, self.evidence)
            .await
            .map_err(guard_failed)?;
        self.check_response(GET_POLICY, &snapshot.policy)?;
        Ok(snapshot)
    }

    pub(super) async fn attachments<O>(&mut self, id: &str) -> Result<Attachments, Failure<O>> {
        let ids = &[("PolicyID", id)];
        let attachments = Attachments {
            groups: self
                .attachment_array("policies-api/v1/policies/{}/groups", ids)
                .await?,
            users: self
                .attachment_array("policies-api/v1/policies/{}/iam-users", ids)
                .await?,
            accounts: self
                .attachment_array("policies-api/v1/policies/{}/service-accounts", ids)
                .await?,
        };
        self.check_ids(attachments.groups.iter().map(|group| group.id.as_str()))?;
        self.check_ids(
            attachments
                .users
                .iter()
                .chain(&attachments.accounts)
                .map(String::as_str),
        )?;
        Ok(attachments)
    }

    async fn attachment_array<T: Wire + Serialize, O>(
        &mut self,
        route: &'static str,
        ids: &[(&str, &str)],
    ) -> Result<Vec<T>, Failure<O>> {
        let list: Option<Vec<T>> = self
            .client
            .fetch(
                self.ctx,
                Api::Policies,
                &read_operation(self.op, route, LIST_CAP, false),
                ids,
                &[],
                self.evidence,
            )
            .await
            .map_err(guard_failed)?;
        let list = list.ok_or_else(|| self.refusal("attachment list was null or missing"))?;
        self.check_response(self.op, &list)?;
        Ok(list)
    }

    pub(super) async fn attached_protection<O>(
        &mut self,
        attachments: Attachments,
    ) -> Result<Protection, Failure<O>> {
        let mut protection = Protection::None;
        for group in attachments.groups {
            protection = protection.max(self.group(&group.id).await?);
            if protection == Protection::Caller {
                return Ok(protection);
            }
        }
        for id in attachments.users {
            protection = protection.max(self.user(&id).await?);
            if protection == Protection::Caller {
                return Ok(protection);
            }
        }
        if self.is_service_account() && !attachments.accounts.is_empty() {
            return Ok(Protection::Caller);
        }
        for id in attachments.accounts {
            let attached = self
                .policy_summaries(
                    "policies-api/v1/user-attachments/service-accounts/{}/policies",
                    "ServiceAccountID",
                    &id,
                )
                .await?;
            if self
                .any_privileged(attached.iter().map(|policy| policy.id.as_str()))
                .await?
            {
                protection = Protection::Privileged;
            }
        }
        Ok(protection)
    }

    pub(super) async fn group<O>(&mut self, id: &str) -> Result<Protection, Failure<O>> {
        let group: GroupAttachments = self
            .client
            .fetch(
                self.ctx,
                Api::Policies,
                &read_operation(self.op, "policies-api/v1/groups/{}", OBJECT_CAP, false),
                &[("GroupID", id)],
                &[],
                self.evidence,
            )
            .await
            .map_err(guard_failed)?;
        let (policies, members) = group.checked(self.op, id, self.evidence)?;
        let mut protection = if self
            .any_privileged(policies.iter().map(String::as_str))
            .await?
        {
            Protection::Privileged
        } else {
            Protection::None
        };
        for id in members {
            protection = protection.max(self.user(&id).await?);
            if protection == Protection::Caller {
                break;
            }
        }
        Ok(protection)
    }

    pub(super) async fn user<O>(&mut self, id: &str) -> Result<Protection, Failure<O>> {
        self.check_response(self.op, &self.caller)?;
        self.check_ids([self.caller.user_id.as_str(), id])?;
        if !self.is_service_account() && id.eq_ignore_ascii_case(&self.caller.user_id) {
            return Ok(Protection::Caller);
        }
        let attached = self
            .policy_summaries(
                "policies-api/v1/user-attachments/iam-users/{}/policies",
                "UserID",
                id,
            )
            .await?;
        if self
            .any_privileged(attached.iter().map(|policy| policy.id.as_str()))
            .await?
        {
            return Ok(Protection::Privileged);
        }
        let groups: Vec<UserGroup> = self
            .attachment_array(
                "policies-api/v1/user-attachments/iam-users/{}/groups",
                &[("UserID", id)],
            )
            .await?;
        if groups.iter().any(|group| group.policies.is_none()) {
            return Err(self.refusal("a user's group had no policies field"));
        }
        for group in groups {
            self.check_ids([group.id.as_str()])?;
            if self
                .any_privileged(group.policies.iter().flatten().map(String::as_str))
                .await?
            {
                return Ok(Protection::Privileged);
            }
        }
        Ok(Protection::None)
    }

    async fn policy_summaries<O>(
        &mut self,
        route: &'static str,
        field: &str,
        id: &str,
    ) -> Result<Vec<PolicySummary>, Failure<O>> {
        let page: AttachmentPage = self
            .client
            .fetch(
                self.ctx,
                Api::Policies,
                &read_operation(self.op, route, LIST_CAP, false),
                &[(field, id)],
                &[("pageNumber", "0"), ("pageSize", "10000")],
                self.evidence,
            )
            .await
            .map_err(guard_failed)?;
        self.check_response(self.op, &page)?;
        let data = page
            .data
            .ok_or_else(|| self.refusal("attachment list response had no data field"))?;
        let total = page
            .total_items
            .ok_or_else(|| self.refusal("attachment list response had no totalItems field"))?;
        if usize::try_from(total).ok() != Some(data.len()) {
            return Err(self.refusal("attachment list item count does not match totalItems"));
        }
        if page.total_pages.is_some_and(|pages| pages > 1) {
            return Err(self.refusal("attachment list reports more than one page"));
        }
        Ok(data)
    }

    async fn any_privileged<'b, O>(
        &mut self,
        ids: impl IntoIterator<Item = &'b str>,
    ) -> Result<bool, Failure<O>> {
        for id in ids {
            self.check_ids([id])?;
            if !self.policies.contains_key(id) {
                let policy = self.policy(id).await?;
                self.policies.insert(id.to_owned(), policy);
            }
            if let Some(policy) = self.policies.get(id)
                && self.privileged(&policy.statements)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn privileged<O>(&self, statements: &[Statement]) -> Result<bool, Failure<O>> {
        self.check_response(ACTIONS, &self.actions)?;
        self.check_response(GET_POLICY, &statements)?;
        Ok(statements.is_empty()
            || statements.iter().any(|statement| {
                !statement.effect.eq_ignore_ascii_case("deny")
                    && (!statement.effect.eq_ignore_ascii_case("allow")
                        || statement.actions.is_empty()
                        || statement
                            .actions
                            .iter()
                            .any(|action| action_is_privileged(action, &self.actions)))
            }))
    }

    fn check_response<O>(&self, op: &str, value: &impl Serialize) -> Result<(), Failure<O>> {
        if serde_json::to_value(value).is_ok_and(|value| !self.evidence.reflected_in(&value)) {
            return Ok(());
        }
        Err(guard_refusal(
            &format!("iam: {op}: unsafe guard response"),
            None,
        ))
    }

    fn check_ids<'b, O>(&self, ids: impl IntoIterator<Item = &'b str>) -> Result<(), Failure<O>> {
        if ids.into_iter().any(|id| self.evidence.id_reflects(id)) {
            return Err(self.refusal("unsafe guard identity"));
        }
        Ok(())
    }

    fn is_service_account(&self) -> bool {
        matches!(self.caller.user_type.as_str(), "user-sa" | "service-sa")
    }

    fn refusal<O>(&self, reason: &str) -> Failure<O> {
        guard_refusal(&format!("iam: guard: {}: {reason}", self.op), None)
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
pub(super) fn protection_error<O>(
    op: &str,
    protection: Protection,
    reason: &str,
) -> Result<(), Failure<O>> {
    match protection {
        Protection::None => Ok(()),
        Protection::Caller => Err(guard_refusal(
            &format!(
                "iam: refused: target is the caller: {op}: the change would affect the caller's own rights"
            ),
            Some(WriteError::SelfChange),
        )),
        Protection::Privileged => Err(guard_refusal(
            &format!("iam: refused: target is protected: {op}: {reason}"),
            Some(WriteError::PrivilegedChange),
        )),
    }
}

fn action_is_privileged(pattern: &str, actions: &[String]) -> bool {
    static VALID: LazyLock<Result<Regex, regex_lite::Error>> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9*]+(-[A-Za-z0-9*]+)*(:[A-Za-z0-9*]+)?$"));
    if !VALID.as_ref().is_ok_and(|valid| valid.is_match(pattern)) {
        return true;
    }
    let pattern = pattern.to_ascii_lowercase();
    if matches!(pattern.as_str(), "*" | "*:*" | "iam:*") {
        return true;
    }
    let expression = format!("^{}$", pattern.replace('*', ".*"));
    Regex::new(&expression).map_or(true, |pattern| {
        actions.iter().any(|action| pattern.is_match(action))
    })
}

#[derive(Default, Serialize)]
struct AttachmentPage {
    data: Option<Vec<PolicySummary>>,
    total_items: Option<i64>,
    total_pages: Option<i64>,
}

impl AttachmentPage {
    const FIELDS: &[&str] = &["data", "totalItems", "totalPages", "pageNumber", "pageSize"];
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
        let fields = raw_fields(raw)?;
        if fields.keys().any(|key| {
            Self::FIELDS
                .iter()
                .any(|field| *field != key && crate::wire::strict_json::field_matches(field, key))
        }) {
            return Err(serde::de::Error::custom(
                "noncanonical attachment page field",
            ));
        }
        Ok(Self {
            data: decode_field(&fields, "data")?,
            total_items: decode_field(&fields, "totalItems")?,
            total_pages: decode_field(&fields, "totalPages")?,
        })
    }
}

#[derive(Default, Serialize)]
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
        let fields = raw_fields(raw)?;
        Ok(Self {
            id: decode_field(&fields, "id")?,
            policies: decode_field(&fields, "policies")?,
        })
    }
}

fn decode_field<T: Wire>(
    fields: &BTreeMap<String, &RawValue>,
    name: &str,
) -> Result<T, serde_json::Error> {
    wire::field(fields, name).map_or_else(|| Ok(T::default()), T::decode)
}

fn raw_fields(raw: &RawValue) -> Result<BTreeMap<String, &RawValue>, serde_json::Error> {
    serde_json::from_str::<Option<_>>(raw.get()).map(Option::unwrap_or_default)
}

#[derive(Default)]
struct PolicySnapshot {
    policy: Policy,
    complete: bool,
    statements: Option<Box<RawValue>>,
}

impl Wire for PolicySnapshot {
    const SHAPE: Shape<'static> = Policy::SHAPE;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = raw_fields(raw)?;
        let complete = ["name", "description"]
            .into_iter()
            .all(|field| wire::field(&fields, field).is_some_and(|value| value.get() != "null"));
        Ok(Self {
            policy: Policy::decode(raw)?,
            complete,
            statements: wire::field(&fields, "statements").map(ToOwned::to_owned),
        })
    }
}

fn preservable_statements(raw: &RawValue) -> Option<Value> {
    let statements: Vec<&RawValue> = serde_json::from_str(raw.get()).ok()?;
    for statement in statements {
        let fields: BTreeMap<String, &RawValue> = serde_json::from_str(statement.get()).ok()?;
        if fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "effect" | "actions" | "resources" | "condition"
            )
        }) {
            return None;
        }
    }
    wire::exact_value(raw).ok()
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    async fn policy_snapshot<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<PolicySnapshot, Failure<O>> {
        let snapshot: PolicySnapshot = self
            .fetch(
                ctx,
                Api::Policies,
                &read_operation(GET_POLICY, POLICY, OBJECT_CAP, false),
                &[("PolicyID", id)],
                &[],
                evidence,
            )
            .await?;
        confirmed_identity(
            &snapshot.policy.id,
            id,
            "iam.GetPolicy: policy identity is unproven",
            evidence,
        )?;
        Ok(snapshot)
    }

    pub(super) async fn confirm_policy<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Policy, Failure<O>> {
        let mut policy = self.policy_snapshot(ctx, id, evidence).await?.policy;
        policy.screen(evidence);
        Ok(policy)
    }
}
