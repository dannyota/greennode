use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::{Error, Failure, RequestContext, WriteDispatch};

use super::inventory::group_read_error;
use super::mutation::{
    Mutation, decode_data, invalid_mutation_response, not_sent, rejected, settlement_error,
    with_screened_advice,
};
use super::quotes::{path_id, required};
use super::wire::{api, invalid_input};
use super::{Client, ListServerGroupsInput, ServerGroup};

const CREATE: Mutation = Mutation {
    id: "compute.CreateServerGroup",
    template: "v2/{}/serverGroups",
    method: Method::POST,
    retry: Retry::NonIdempotent,
    overflow_overrides_status: false,
    success: &[StatusCode::CREATED],
};
const UPDATE: Mutation = Mutation {
    id: "compute.UpdateServerGroup",
    template: "v2/{}/serverGroups/{}",
    method: Method::PUT,
    retry: Retry::Repeatable,
    overflow_overrides_status: false,
    success: &[StatusCode::OK],
};
const DELETE: Mutation = Mutation {
    id: "compute.DeleteServerGroup",
    template: "v2/{}/serverGroups/{}",
    method: Method::DELETE,
    retry: Retry::Repeatable,
    overflow_overrides_status: false,
    success: &[StatusCode::NO_CONTENT],
};

/// The requested server group is in use.
#[derive(Clone, Copy)]
pub struct ServerGroupInUse;

crate::error::debug_record!(ServerGroupInUse unit);

/// Name, placement policy and description for a new server group.
#[derive(Clone, Default)]
pub struct CreateServerGroupInput {
    /// Unique group name within the project.
    pub name: String,
    /// Placement policy.
    pub policy_id: String,
    /// Group description.
    pub description: String,
}

crate::error::debug_record!(CreateServerGroupInput {
    name,
    policy_id,
    description
});

/// Mutable fields of a server group. Omitted fields retain their current values.
#[derive(Clone, Default)]
pub struct UpdateServerGroupInput {
    /// Group to update.
    pub server_group_id: String,
    /// New nonempty group name.
    pub name: Option<String>,
    /// New description. An empty value clears it.
    pub description: Option<String>,
}

crate::error::debug_record!(UpdateServerGroupInput {
    server_group_id,
    name,
    description
});

/// Identifies a server group to delete.
#[derive(Clone, Default)]
pub struct DeleteServerGroupInput {
    /// Group to delete.
    pub server_group_id: String,
}

crate::error::debug_record!(DeleteServerGroupInput { server_group_id });

/// The group returned by a successful create.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateServerGroupOutput {
    /// The newly created group.
    pub server_group: ServerGroup,
}

crate::wire::product::output::public_fields!(CreateServerGroupOutput { server_group });

/// The group read after an update.
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateServerGroupOutput {
    /// The confirmed group, or its requested fields on an unconfirmed update.
    pub server_group: ServerGroup,
}

crate::wire::product::output::public_fields!(UpdateServerGroupOutput { server_group });

/// An acknowledgement of server-group deletion.
#[derive(Clone, Default, Serialize)]
pub struct DeleteServerGroupOutput {}

crate::wire::product::output::public_fields!(DeleteServerGroupOutput {});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a server group using the non-idempotent request policy.
    pub async fn create_server_group(
        &self,
        ctx: &RequestContext,
        input: CreateServerGroupInput,
    ) -> Result<CreateServerGroupOutput, Failure<CreateServerGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            required(
                CREATE.id,
                &[
                    ("Name", !input.name.is_empty()),
                    ("PolicyID", !input.policy_id.is_empty()),
                ],
            )
            .map_err(|error| not_sent(*error))?;
            path_id(CREATE.id, "PolicyID", &input.policy_id).map_err(|error| not_sent(*error))?;
            let mut evidence = Evidence::for_write();
            let body = json!({"name": input.name, "policyId": input.policy_id, "description": input.description});
            self.guard_write(ctx, &body, &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let raw = self
                .send_mutation(ctx, &CREATE, &[], Some(&body), false, &mut evidence)
                .await
                .map_err(|error| ambiguous_create(error, &evidence))?;
            let receipt = raw.clean_receipt().ok_or_else(|| {
                ambiguous_create(
                    Failure::new(
                        invalid_mutation_response(&CREATE, &evidence),
                        None,
                        WriteDispatch::MayHaveBeenSent,
                    ),
                    &evidence,
                )
            })?;
            let mut group = decode_data::<ServerGroup>(receipt.body(), &CREATE, &evidence)
                .map_err(|error| {
                    ambiguous_create(
                        Failure::new(*error, None, WriteDispatch::MayHaveBeenSent),
                        &evidence,
                    )
                })?;
            if group.uuid.is_empty() {
                return Err(Failure::new(
                    api(
                        CREATE.id,
                        Some(201),
                        "",
                        "create response had no id; the group may exist, list server groups and match the name exactly before creating it again",
                        &evidence,
                    ),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            if !PathIds::WithUnderscore.allows(&group.uuid) || evidence.id_reflects(&group.uuid) {
                return Err(Failure::new(
                    api(
                        CREATE.id,
                        Some(201),
                        "InvalidResponse",
                        "create response had no usable id; the group may exist, list server groups and match the name exactly before creating it again",
                        &evidence,
                    ),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            if !evidence.can_retain_partial()
                || group_ids_reflect(&group, &evidence)
                || !group.redact(&evidence)
            {
                return Err(Failure::new(
                    invalid_mutation_response(&CREATE, &evidence),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            Ok(CreateServerGroupOutput {
                server_group: group,
            })
        })
    }

    /// Merges and applies the group's mutable fields, then reads back the group.
    pub async fn update_server_group(
        &self,
        ctx: &RequestContext,
        input: UpdateServerGroupInput,
    ) -> Result<UpdateServerGroupOutput, Failure<UpdateServerGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.server_group_id;
            path_id(UPDATE.id, "ServerGroupID", id).map_err(|error| not_sent(*error))?;
            if input.name.is_none() && input.description.is_none() {
                return Err(not_sent(invalid_input(
                    "compute.UpdateServerGroup requires at least one field to change",
                )));
            }
            if input.name.as_deref() == Some("") {
                return Err(not_sent(invalid_input(
                    "compute.UpdateServerGroup: Name must not be empty",
                )));
            }
            let mut evidence = Evidence::for_write();
            self.guard_write(
                ctx,
                &json!([id, input.name, input.description]),
                &mut evidence,
            )
            .map_err(|error| not_sent(*error))?;
            let mut group = self
                .group_detail_checked::<()>(ctx, id, &mut evidence, true)
                .await
                .map_err(|error| not_sent(error.into_parts().0))?;
            if group.uuid != *id
                || group_ids_reflect(&group, &evidence)
                || serde_json::to_value(&group)
                    .map_or(true, |value| evidence.output_reflects(&value))
                || !group.redact(&evidence)
            {
                return Err(not_sent(group_read_error(
                    "compute.GetServerGroup",
                    &evidence,
                )));
            }
            let mut fallback = ServerGroup {
                uuid: id.clone(),
                name: input.name.unwrap_or(group.name),
                description: input.description.unwrap_or(group.description),
                ..ServerGroup::default()
            };
            let body = json!({"name": fallback.name, "description": fallback.description, "serverGroupId": id});
            self.send_mutation(ctx, &UPDATE, &[id], Some(&body), false, &mut evidence)
                .await
                .map(drop)?;
            let result = self.group_detail::<()>(ctx, id, &mut evidence).await;
            let error = match result {
                Ok(mut group)
                    if group.uuid == *id
                        && !group_ids_reflect(&group, &evidence)
                        && evidence.can_retain_partial() =>
                {
                    if group.redact(&evidence) {
                        return Ok(UpdateServerGroupOutput {
                            server_group: group,
                        });
                    }
                    group_read_error("compute.GetServerGroup", &evidence)
                }
                Ok(_) => group_read_error("compute.GetServerGroup", &evidence),
                Err(error) => error.into_parts().0,
            };
            let partial = (evidence.can_retain_partial()
                && !group_ids_reflect(&fallback, &evidence)
                && fallback.redact(&evidence))
            .then_some(UpdateServerGroupOutput {
                server_group: fallback,
            });
            Err(Failure::new(
                settlement_error(
                    UPDATE.id,
                    error,
                    "read get-server-group to confirm the update",
                    &evidence,
                ),
                partial,
                WriteDispatch::MayHaveBeenSent,
            ))
        })
    }

    /// Deletes a group after checking its listed membership.
    pub async fn delete_server_group(
        &self,
        ctx: &RequestContext,
        input: DeleteServerGroupInput,
    ) -> Result<DeleteServerGroupOutput, Failure<DeleteServerGroupOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.server_group_id;
            path_id(DELETE.id, "ServerGroupID", id).map_err(|error| not_sent(*error))?;
            let mut evidence = Evidence::for_write();
            self.guard_write(ctx, &json!(id), &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let groups = self
                .group_page_checked::<()>(
                    ctx,
                    &ListServerGroupsInput::default(),
                    &mut evidence,
                    true,
                )
                .await
                .map_err(|error| not_sent(error.into_parts().0))?;
            if evidence.output_reflects(&json!([
                groups.page,
                groups.page_size,
                groups.total_page,
                groups.total_item
            ])) {
                return Err(not_sent(group_read_error(
                    "compute.ListServerGroups",
                    &evidence,
                )));
            }
            for mut group in groups.items {
                if !PathIds::WithUnderscore.allows(&group.uuid)
                    || group_ids_reflect(&group, &evidence)
                    || !group.redact(&evidence)
                {
                    return Err(not_sent(group_read_error(
                        "compute.ListServerGroups",
                        &evidence,
                    )));
                }
                if group.uuid == *id
                    && let Some(members) = group.servers
                    && !members.is_empty()
                {
                    let count = members.len();
                    let message = format!(
                        "compute: server group in use: {}: server group {id} has {count} server(s) attached",
                        DELETE.id
                    );
                    return Err(not_sent(in_use(&message, &evidence)));
                }
            }
            self.send_mutation::<()>(ctx, &DELETE, &[id], None, false, &mut evidence)
                .await
                .map(drop)
                .map_err(|failure| {
                    let (error, _, dispatch) = failure.into_parts();
                    let error = if let Some(api) = error.api_error()
                        && api
                            .message()
                            .to_ascii_lowercase()
                            .contains("server group is in use")
                    {
                        let message = format!("compute: server group in use: {}", api.message());
                        error.with_cause(in_use(&message, &evidence))
                    } else {
                        error
                    };
                    Failure::new(error, None, dispatch)
                })?;
            Ok(DeleteServerGroupOutput {})
        })
    }
}

fn group_ids_reflect(group: &ServerGroup, evidence: &Evidence) -> bool {
    evidence.id_reflects(&group.uuid)
        || evidence.id_reflects(&group.policy_id)
        || evidence.identifier_output_reflects(&group.server_group_id)
        || group.servers.as_ref().is_some_and(|servers| {
            servers
                .iter()
                .any(|server| evidence.id_reflects(&server.uuid))
        })
}

fn in_use(message: &str, evidence: &Evidence) -> Error {
    api("", None, "ServerGroupInUse", message, evidence).with_detail(ServerGroupInUse)
}

fn ambiguous_create(
    failure: Failure<CreateServerGroupOutput>,
    evidence: &Evidence,
) -> Failure<CreateServerGroupOutput> {
    let (error, _, dispatch) = failure.into_parts();
    let error = if dispatch == WriteDispatch::MayHaveBeenSent && !rejected(&error) {
        let reason = error
            .api_error()
            .map_or_else(String::new, crate::ApiError::go_text);
        let advice = format!(
            "{}: create may have already reached the server; list server groups and match the name exactly before creating it again: {reason}",
            CREATE.id
        );
        with_screened_advice(error, &advice, evidence)
    } else {
        error
    };
    Failure::new(error, None, dispatch)
}
