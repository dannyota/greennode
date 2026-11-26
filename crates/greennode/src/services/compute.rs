mod prices;
mod server_commands;

use clap::ArgMatches;
use greennode_sdk::compute as sdk;

use crate::cli::dispatch;
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

use crate::cli::dispatch::{OutputView, RawText};

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! string_field {
    ($input:ty, $field:ident, $flag:ident, $name:literal, $requirement:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut $input, value| {
                let serde_json::Value::String(value) = value else {
                    return Err(());
                };
                input.$field = value;
                Ok(())
            },
            set_flag: Some(|input: &mut $input, matches| {
                input.$field = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

macro_rules! required_id {
    ($input:ty, $field:ident, $flag:ident, $name:literal) => {
        string_field!(
            $input,
            $field,
            $flag,
            $name,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
}

mod server_actions;
mod server_groups;

macro_rules! int_field {
    ($field:ident, $flag:ident, $name:literal) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! operation {
    ($input:ident, $defaults:expr, $method:ident, $name:literal, $go:literal,
     $help:literal, $notes:expr, $console:expr, [$($flag:ident => $field:expr),* $(,)?]) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: $defaults,
            fields: &[$($field),*],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: None,
        };
        Operation {
            name: $name,
            sdk_operation: concat!("compute.", $go),
            short_help: $help,
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[$($flag),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                let view = if $console {
                    console_view(invocation.stdout_terminal)
                } else {
                    OutputView::default()
                };
                Box::pin(dispatch::management_with_view(
                    invocation, operation, matches, &INPUT,
                    |ctx, config, input| Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, |output| project!($method, output))
                    }),
                    view,
                ))
            })),
        }
    }};
}

macro_rules! project {
    (get_server_console_log, $output:ident) => {
        ConsoleLog {
            log: $output.log.expose_secret().to_owned(),
        }
    };
    ($operation:ident, $output:ident) => {
        $output
    };
}

#[derive(serde::Serialize)]
struct ConsoleLog {
    #[serde(rename = "Log")]
    log: String,
}

fn console_view(terminal: bool) -> OutputView {
    OutputView {
        raw_text: Some(RawText {
            terminal,
            unqueried_field: "Log",
        }),
        withhold: Some("console log query failed; result withheld"),
        ..OutputView::default()
    }
}

const SERVER_NOTE: &str = "PublicIP is the canonical floating address derived from internal interfaces. Conflicting, invalid, private, or absent addresses leave it empty.";
const CONSOLE_NOTE: &str = "Console logs can hold passwords and keys. Text output writes the log without an added newline. Terminals escape controls other than newline and tab. A query that returns a string uses the same text rules.";

const SERVER_ID: Field = flag("server-id", "string", true);
const SERVER_GROUP_ID: Field = flag("server-group-id", "string", true);
const SSH_KEY_ID: Field = flag("ssh-key-id", "string", true);
const ZONE_ID: Field = flag("zone-id", "string", false);
const FLAVOR_ZONE_ID: Field = flag("flavor-zone-id", "string", false);
const NAME: Field = flag("name", "string", false);
const OFFSET: Field = flag("offset", "int", false);
const LIMIT: Field = flag("limit", "int", false);
const PAGE: Field = flag("page", "int", false);
const SIZE: Field = flag("size", "int", false);

pub(super) const SERVICE: Service = Service {
    name: "compute",
    error_rules: &[
        prices::ERROR_RULES[0],
        server_actions::UNEXPECTED_STATUS,
        server_groups::GROUP_IN_USE,
    ],
    operations: &[
        server_commands::CREATE_SERVER,
        server_commands::DELETE_SERVER,
        server_commands::QUOTE_CREATE_SERVER,
        server_commands::QUOTE_RESIZE_SERVER,
        server_actions::START,
        server_actions::STOP,
        server_actions::REBOOT,
        server_actions::RENAME,
        server_actions::RESIZE,
        server_groups::CREATE,
        server_groups::UPDATE,
        server_groups::DELETE,
        operation!(
            GetServerInput, || sdk::GetServerInput::new(""), get_server, "get-server", "GetServer",
            "Get server", &[SERVER_NOTE], false, [
                SERVER_ID => required_id!(sdk::GetServerInput, server_id, SERVER_ID, "ServerID"),
            ]
        ),
        operation!(
            GetServerConsoleLogInput, || sdk::GetServerConsoleLogInput::new(""), get_server_console_log, "get-server-console-log", "GetServerConsoleLog",
            "Read the server console log. Logs can hold passwords and keys.", &[CONSOLE_NOTE], true, [
                SERVER_ID => required_id!(sdk::GetServerConsoleLogInput, server_id, SERVER_ID, "ServerID"),
            ]
        ),
        operation!(
            GetServerGroupInput, || sdk::GetServerGroupInput::new(""), get_server_group, "get-server-group", "GetServerGroup",
            "Get server group", &[], false, [
                SERVER_GROUP_ID => required_id!(sdk::GetServerGroupInput, server_group_id, SERVER_GROUP_ID, "ServerGroupID"),
            ]
        ),
        operation!(
            GetSshKeyInput, || sdk::GetSshKeyInput::new(""), get_ssh_key, "get-ssh-key", "GetSSHKey",
            "Get ssh key", &[], false, [
                SSH_KEY_ID => required_id!(sdk::GetSshKeyInput, ssh_key_id, SSH_KEY_ID, "SSHKeyID"),
            ]
        ),
        prices::OPERATION,
        operation!(
            ListFlavorZonesInput, sdk::ListFlavorZonesInput::default, list_flavor_zones, "list-flavor-zones", "ListFlavorZones",
            "List flavor zones", &[], false, [
                ZONE_ID => string_field!(sdk::ListFlavorZonesInput, zone_id, ZONE_ID, "ZoneID", Requirement::Optional),
            ]
        ),
        operation!(
            ListFlavorsInput, sdk::ListFlavorsInput::default, list_flavors, "list-flavors", "ListFlavors",
            "List flavors", &[], false, [
                FLAVOR_ZONE_ID => string_field!(sdk::ListFlavorsInput, flavor_zone_id, FLAVOR_ZONE_ID, "FlavorZoneID", Requirement::Optional),
                ZONE_ID => string_field!(sdk::ListFlavorsInput, zone_id, ZONE_ID, "ZoneID", Requirement::Optional),
                NAME => string_field!(sdk::ListFlavorsInput, name, NAME, "Name", Requirement::Optional),
            ]
        ),
        operation!(
            ListGpuImagesInput,
            sdk::ListGpuImagesInput::default,
            list_gpu_images,
            "list-gpu-images",
            "ListGPUImages",
            "List gpu images",
            &[],
            false,
            []
        ),
        operation!(
            ListOsImagesInput, sdk::ListOsImagesInput::default, list_os_images, "list-os-images", "ListOSImages",
            "List os images", &[], false, [
                ZONE_ID => string_field!(sdk::ListOsImagesInput, zone_id, ZONE_ID, "ZoneID", Requirement::Optional),
            ]
        ),
        operation!(
            ListServerGroupMembersInput,
            sdk::ListServerGroupMembersInput::default,
            list_server_group_members,
            "list-server-group-members",
            "ListServerGroupMembers",
            "List server group members",
            &[],
            false,
            []
        ),
        operation!(
            ListServerGroupPoliciesInput,
            sdk::ListServerGroupPoliciesInput::default,
            list_server_group_policies,
            "list-server-group-policies",
            "ListServerGroupPolicies",
            "List server group policies",
            &[],
            false,
            []
        ),
        operation!(
            ListServerGroupsInput, sdk::ListServerGroupsInput::default, list_server_groups, "list-server-groups", "ListServerGroups",
            "List server groups", &[], false, [
                NAME => string_field!(sdk::ListServerGroupsInput, name, NAME, "Name", Requirement::Optional),
                OFFSET => int_field!(offset, OFFSET, "Offset"),
                LIMIT => int_field!(limit, LIMIT, "Limit"),
            ]
        ),
        operation!(
            ListServerSecurityGroupsInput,
            sdk::ListServerSecurityGroupsInput::default,
            list_server_security_groups,
            "list-server-security-groups",
            "ListServerSecurityGroups",
            "List server security groups",
            &[],
            false,
            []
        ),
        operation!(
            ListServersInput, sdk::ListServersInput::default, list_servers, "list-servers", "ListServers",
            "List servers", &[SERVER_NOTE], false, [
                PAGE => int_field!(page, PAGE, "Page"),
                SIZE => int_field!(size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListSshKeysInput, sdk::ListSshKeysInput::default, list_ssh_keys, "list-ssh-keys", "ListSSHKeys",
            "List ssh keys", &[], false, [
                NAME => string_field!(sdk::ListSshKeysInput, name, NAME, "Name", Requirement::Optional),
                PAGE => int_field!(page, PAGE, "Page"),
                SIZE => int_field!(size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListUserImagesInput, sdk::ListUserImagesInput::default, list_user_images, "list-user-images", "ListUserImages",
            "List user images", &[], false, [
                PAGE => int_field!(page, PAGE, "Page"),
                SIZE => int_field!(size, SIZE, "Size"),
            ]
        ),
    ],
};
