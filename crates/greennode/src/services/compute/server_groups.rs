use super::server_commands::{execute, server_operation};
use super::*;
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};

pub(super) const GROUP_IN_USE: Rule = Rule {
    identity: "compute-server-group-in-use",
    matches: |error| error.detail::<sdk::ServerGroupInUse>().is_some(),
    name: Some(RuleName {
        rank: 29,
        code: "ServerGroupInUse",
        operation: RuleOperation::Omit,
        include_api_status: false,
    }),
    exit: Some(RuleExit::Failure),
    message: None,
};

const GROUP_NAME: Field = flag("name", "string", true);
const POLICY_ID: Field = flag("policy-id", "string", true);
const DESCRIPTION: Field = flag("description", "string", false);
const NEW_NAME: Field = flag("name", "*string", false);
const NEW_DESCRIPTION: Field = flag("description", "*string", false);

macro_rules! optional_text {
    ($name:literal, $field:ident, $flag:ident) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut sdk::UpdateServerGroupInput, value| {
                input.$field = Some(value.as_str().ok_or(())?.to_owned());
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$field = Some(matches.get_one::<String>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

execute!(create_server_group, sdk::CreateServerGroupInput);
server_operation!(
    CREATE,
    "create-server-group",
    "compute.CreateServerGroup",
    Write,
    "Create server group",
    "Creates a group with a placement policy. Inspect `list-server-groups` after an unconfirmed create.",
    sdk::CreateServerGroupInput,
    create_server_group,
    &[GROUP_NAME, POLICY_ID, DESCRIPTION],
    &[],
    &[],
    [
        required_id!(sdk::CreateServerGroupInput, name, GROUP_NAME, "Name"),
        required_id!(
            sdk::CreateServerGroupInput,
            policy_id,
            POLICY_ID,
            "PolicyID"
        ),
        string_field!(
            sdk::CreateServerGroupInput,
            description,
            DESCRIPTION,
            "Description",
            Requirement::Optional
        ),
    ]
);

execute!(update_server_group, sdk::UpdateServerGroupInput);
server_operation!(
    UPDATE,
    "update-server-group",
    "compute.UpdateServerGroup",
    Write,
    "Update server group",
    "Changes the group name or description. An empty description clears it.",
    sdk::UpdateServerGroupInput,
    update_server_group,
    &[SERVER_GROUP_ID, NEW_NAME, NEW_DESCRIPTION],
    &[],
    &[],
    [
        required_id!(
            sdk::UpdateServerGroupInput,
            server_group_id,
            SERVER_GROUP_ID,
            "ServerGroupID"
        ),
        optional_text!("Name", name, NEW_NAME),
        optional_text!("Description", description, NEW_DESCRIPTION),
    ]
);

execute!(delete_server_group, sdk::DeleteServerGroupInput);
server_operation!(
    DELETE,
    "delete-server-group",
    "compute.DeleteServerGroup",
    DestructiveWrite,
    "Delete server group",
    "Checks group membership before requesting deletion.",
    sdk::DeleteServerGroupInput,
    delete_server_group,
    &[SERVER_GROUP_ID],
    &[],
    &[],
    [required_id!(
        sdk::DeleteServerGroupInput,
        server_group_id,
        SERVER_GROUP_ID,
        "ServerGroupID"
    ),]
);
