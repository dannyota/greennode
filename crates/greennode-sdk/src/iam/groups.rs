use std::collections::BTreeMap;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, json, value::RawValue};

use super::wire::{self, Screen, Wire};
use super::writes::{guard_failed, guard_refusal, settle, write_operation};
use super::{Api, Client, Group, OBJECT_CAP, invalid_input, read_operation};
use crate::error::RecoveryAdvice;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::Operation,
    options::{Options, PathIds},
    redirect::Retry,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{ApiError, Error, Failure, RequestContext, WriteDispatch};

const GROUPS: &str = "policies-api/v1/groups";
const GROUP: &str = "policies-api/v1/groups/{}";
const CREATE: &str = "iam.CreateGroup";
const UPDATE: &str = "iam.UpdateGroup";
const DELETE: &str = "iam.DeleteGroup";

/// Creates an IAM-mode group with no members or policies.
#[derive(Clone)]
pub struct CreateGroupInput {
    /// Required group name.
    pub name: String,
    /// Group description.
    pub description: String,
}

crate::error::debug_record!(CreateGroupInput { name, description });

impl CreateGroupInput {
    /// Sets the name and leaves the description empty.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
        }
    }
}

/// Group returned by a create or its confirmation failure.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateGroupOutput {
    /// The confirmed group, or only its ID when confirmation failed.
    #[serde(rename = "Group")]
    pub group: Group,
}

crate::wire::product::output::public_fields!(CreateGroupOutput { group });

/// Updates a group's name or description.
#[derive(Clone)]
pub struct UpdateGroupInput {
    /// Group ID using ASCII letters, digits, or hyphens.
    pub group_id: String,
    /// When absent, reads the current group name before updating.
    pub name: Option<String>,
    /// When absent, omits the description from the update.
    pub description: Option<String>,
}

crate::error::debug_record!(UpdateGroupInput {
    group_id,
    name,
    description
});

impl UpdateGroupInput {
    /// Selects the group and preserves its name and description by default.
    pub fn new(group_id: impl Into<String>) -> Self {
        Self {
            group_id: group_id.into(),
            name: None,
            description: None,
        }
    }
}

/// Group returned by an update or its confirmation failure.
#[derive(Clone, PartialEq, Serialize)]
pub struct UpdateGroupOutput {
    /// The confirmed group, the pre-write read, or the supplied ID.
    #[serde(rename = "Group")]
    pub group: Group,
}

crate::wire::product::output::public_fields!(UpdateGroupOutput { group });

super::id_input!(
    /// Identifies an empty IAM group to delete.
    DeleteGroupInput,
    /// Group ID using ASCII letters, digits, or hyphens.
    group_id
);

/// A completed group deletion.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DeleteGroupOutput {}

crate::wire::product::output::public_fields!(DeleteGroupOutput {});

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Creates a group once and reads it back using its returned ID.
    pub async fn create_group(
        &self,
        ctx: &RequestContext,
        input: CreateGroupInput,
    ) -> Result<CreateGroupOutput, Failure<CreateGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.name.is_empty() {
                return Err(guard_failed(invalid_input(format!(
                    "{CREATE} requires Name"
                ))));
            }
            let mut evidence =
                self.write_evidence(ctx, CREATE, &[], &[&input.name, &input.description])?;
            let operation = Operation {
                keep_status: true,
                ..write_operation(
                    CREATE,
                    Method::POST,
                    GROUPS,
                    Retry::Once,
                    &[StatusCode::CREATED],
                )
            };
            let body = json!({"name": input.name, "description": input.description, "mode": "iam"});
            let id = self
                .write::<CreatedId, _>(
                    ctx,
                    Api::Policies,
                    &operation,
                    &[],
                    Some(body),
                    &mut evidence,
                )
                .await
                .map_err(|failed| create_failed(failed, &evidence))?
                .0;
            if !PathIds::WithUnderscore.allows(&id) || evidence.id_reflects(&id) {
                return Err(Failure::new(
                    Error::from_api(ApiError::without_fallback(
                        CREATE,
                        None,
                        SafeText::new("", &[]),
                        SafeText::new(
                            "create response had no id; a group may exist, check with list-groups",
                            &[],
                        ),
                        false,
                    )),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            let confirmed = if Options::DEFAULT.path_ids.allows(&id) {
                self.fetch_group(ctx, &id, false, &mut evidence).await
            } else {
                let shown_id = id.get(..64).unwrap_or(&id);
                Err(invalid_input(format!(
                    "iam.GetGroup requires GroupID to match ^[A-Za-z0-9-]+$, got {shown_id:?}"
                )))
            };
            let group = confirmed.map_err(|failed| {
                let mut group = Group::default();
                group.id.clone_from(&id);
                group.screen(&evidence);
                settle(
                    failed,
                    (!evidence.id_reflects(&id)).then_some(CreateGroupOutput { group }),
                    &format!("{CREATE}: group {id} was created but the read to confirm it failed"),
                    &evidence,
                )
            })?;
            Ok(CreateGroupOutput { group })
        })
    }

    /// Updates group metadata and reads the group back after acceptance.
    pub async fn update_group(
        &self,
        ctx: &RequestContext,
        input: UpdateGroupInput,
    ) -> Result<UpdateGroupOutput, Failure<UpdateGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.group_id.as_str();
            let mut evidence = self.write_evidence(
                ctx,
                UPDATE,
                &[("GroupID", id)],
                &[
                    input.name.as_deref().unwrap_or_default(),
                    input.description.as_deref().unwrap_or_default(),
                ],
            )?;
            let mut fallback = Group::default();
            fallback.id = id.to_owned();
            let name = match input.name {
                Some(name) => name,
                None => {
                    fallback = self
                        .fetch_group(ctx, id, true, &mut evidence)
                        .await
                        .map_err(guard_failed)?;
                    fallback.name.clone()
                }
            };
            let mut body = serde_json::Map::new();
            body.insert("name".into(), Value::from(name));
            if let Some(description) = input.description {
                body.insert("description".into(), Value::from(description));
            }
            let op = write_operation(
                UPDATE,
                Method::PATCH,
                GROUP,
                Retry::Repeatable,
                &[StatusCode::NO_CONTENT],
            );
            self.write::<Value, _>(
                ctx,
                Api::Policies,
                &op,
                &[id],
                Some(Value::Object(body)),
                &mut evidence,
            )
            .await?;
            let group = self
                .fetch_group(ctx, id, false, &mut evidence)
                .await
                .map_err(|failed| {
                    let safe_id = !evidence.id_reflects(id) && !evidence.id_reflects(&fallback.id);
                    fallback.screen(&evidence);
                    settle(
                        failed,
                        safe_id.then_some(UpdateGroupOutput { group: fallback }),
                        &format!(
                            "{UPDATE}: group {id} was updated but the read to confirm it failed"
                        ),
                        &evidence,
                    )
                })?;
            Ok(UpdateGroupOutput { group })
        })
    }

    /// Deletes an IAM group only after proving it has no users or policies.
    pub async fn delete_group(
        &self,
        ctx: &RequestContext,
        input: DeleteGroupInput,
    ) -> Result<DeleteGroupOutput, Failure<DeleteGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.group_id.as_str();
            let ids = &[("GroupID", id)];
            let mut evidence = self.write_evidence(ctx, DELETE, ids, &[])?;
            self.require_classified_caller(ctx, DELETE, &mut evidence)
                .await?;
            let group: GroupAttachments = self
                .fetch(
                    ctx,
                    Api::Policies,
                    &read_operation(DELETE, GROUP, OBJECT_CAP, false),
                    ids,
                    &[],
                    &mut evidence,
                )
                .await
                .map_err(guard_failed)?;
            group.require_empty(id, &evidence)?;
            let op = write_operation(
                DELETE,
                Method::DELETE,
                GROUP,
                Retry::Repeatable,
                &[StatusCode::NO_CONTENT],
            );
            self.write::<Value, _>(ctx, Api::Policies, &op, &[id], None, &mut evidence)
                .await?;
            Ok(DeleteGroupOutput {})
        })
    }

    pub(super) async fn fetch_group<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        require_name: bool,
        evidence: &mut Evidence,
    ) -> Result<Group, Failure<O>> {
        let GroupRead {
            mut group,
            has_name,
        } = self
            .fetch(
                ctx,
                Api::Policies,
                &read_operation("iam.GetGroup", GROUP, OBJECT_CAP, false),
                &[("GroupID", id)],
                &[],
                evidence,
            )
            .await?;
        let message = "iam.GetGroup: group identity or required name is unproven";
        super::require_identity(&group.id, id, message)?;
        if require_name && (!has_name || evidence.text_reflects(&group.name)) {
            return Err(guard_refusal(message, None));
        }
        group.screen(evidence);
        Ok(group)
    }
}

fn create_failed<O>(failed: Failure<O>, evidence: &Evidence) -> Failure<O> {
    let (error, _, dispatch) = failed.into_parts();
    if dispatch == WriteDispatch::NotSent
        || error
            .api_error()
            .and_then(|api| api.status())
            .is_some_and(|status| (400..500).contains(&status))
    {
        return Failure::new(error, None, dispatch);
    }
    let advice = format!(
        "iam.CreateGroup: create may have already reached the server; check with list-groups before creating it again: {}",
        super::writes::error_reason(&error),
    );
    let error = if evidence.text_reflects(&advice) {
        error
    } else {
        error.with_outer_detail(RecoveryAdvice::new(&advice))
    };
    Failure::new(error, None, dispatch)
}

#[derive(Default)]
pub(super) struct CreatedId(pub(super) String);

impl Wire for CreatedId {
    const SHAPE: Shape<'static> = Shape::Object(&[ShapeField {
        name: "id",
        shape: Shape::Any,
    }]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str::<Option<BTreeMap<String, &RawValue>>>(raw.get())?
            .unwrap_or_default();
        Ok(Self(
            wire::field(&fields, "id")
                .map(String::decode)
                .transpose()?
                .unwrap_or_default(),
        ))
    }
}

#[derive(Default)]
struct GroupRead {
    group: Group,
    has_name: bool,
}

impl Wire for GroupRead {
    const SHAPE: Shape<'static> = Group::SHAPE;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str::<Option<BTreeMap<String, &RawValue>>>(raw.get())?
            .unwrap_or_default();
        let has_name = wire::field(&fields, "name").is_some_and(|raw| raw.get() != "null");
        Ok(Self {
            group: Group::decode(raw)?,
            has_name,
        })
    }

    fn scan_operands(
        &self,
        scan: &crate::wire::product::output::FieldScan<'_>,
        identifier: bool,
    ) -> bool {
        self.group.scan_operands(scan, identifier)
    }
}

#[derive(Default)]
pub(super) struct GroupAttachments {
    id: String,
    mode: String,
    iam_users: Option<Vec<String>>,
    policies: Option<Vec<String>>,
    unrecognized: bool,
}

impl Wire for GroupAttachments {
    const SHAPE: Shape<'static> = Group::SHAPE;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str::<Option<BTreeMap<String, &RawValue>>>(raw.get())?
            .unwrap_or_default();
        let unrecognized = fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "id" | "name"
                    | "description"
                    | "mode"
                    | "root"
                    | "iamUsers"
                    | "policies"
                    | "createdAt"
                    | "_id"
            )
        });
        Ok(Self {
            id: wire::field(&fields, "id")
                .map(String::decode)
                .transpose()?
                .unwrap_or_default(),
            mode: wire::field(&fields, "mode")
                .map(String::decode)
                .transpose()?
                .unwrap_or_default(),
            iam_users: wire::field(&fields, "iamUsers")
                .map(Option::<Vec<String>>::decode)
                .transpose()?
                .flatten(),
            policies: wire::field(&fields, "policies")
                .map(Option::<Vec<String>>::decode)
                .transpose()?
                .flatten(),
            unrecognized,
        })
    }

    fn scan_operands(&self, scan: &crate::wire::product::output::FieldScan<'_>, _: bool) -> bool {
        self.id.scan_operands(scan, true)
            || self.iam_users.scan_operands(scan, true)
            || self.policies.scan_operands(scan, true)
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl GroupAttachments {
    fn require_empty<O>(self, id: &str, evidence: &Evidence) -> Result<(), Failure<O>> {
        let (policies, users) = self.checked(DELETE, id, evidence)?;
        if !policies.is_empty() || !users.is_empty() {
            return Err(super::writes::proved_refusal(
                evidence,
                super::writes::ProofRefusal::InUse,
                "iam: refused: resource in use: iam.DeleteGroup: the group has a member or a policy",
            ));
        }
        Ok(())
    }

    pub(super) fn checked<O>(
        self,
        op: &str,
        id: &str,
        evidence: &Evidence,
    ) -> Result<(Vec<String>, Vec<String>), Failure<O>> {
        let refusal = |reason| guard_refusal(&format!("iam: guard: {op}: {reason}"), None);
        if self.unrecognized {
            return Err(refusal("a group response held an unrecognized field"));
        }
        if self.mode != "iam" || evidence.text_reflects(&self.mode) {
            return Err(refusal("the group is not in iam mode"));
        }
        let policies = self
            .policies
            .ok_or_else(|| refusal("a group response had no policies field"))?;
        let users = self
            .iam_users
            .ok_or_else(|| refusal("a group response had no iamUsers field"))?;
        if !Options::DEFAULT.path_ids.allows(&self.id)
            || !self.id.eq_ignore_ascii_case(id)
            || evidence.id_reflects(&self.id)
            || evidence.id_reflects(id)
        {
            return Err(guard_refusal(
                &format!("{op}: group identity is unproven"),
                None,
            ));
        }
        if policies
            .iter()
            .chain(&users)
            .any(|id| evidence.id_reflects(id))
        {
            return Err(refusal("unsafe group attachments"));
        }

        Ok((policies, users))
    }
}
