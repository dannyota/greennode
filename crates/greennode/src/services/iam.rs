use clap::ArgMatches;
use greennode_sdk::iam as sdk;

use crate::cli::dispatch;
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const NAME: Field = flag("name", "string", false);
const REQUIRED_NAME: Field = flag("name", "string", true);
const OPTIONAL_NAME: Field = flag("name", "*string", false);
const DESCRIPTION: Field = flag("description", "string", false);
const OPTIONAL_DESCRIPTION: Field = flag("description", "*string", false);
const PAGE: Field = flag("page", "int", false);
const SIZE: Field = flag("size", "int", false);
const GROUP_ID: Field = flag("group-id", "string", true);
const POLICY_ID: Field = flag("policy-id", "string", true);
const SERVICE_ACCOUNT_ID: Field = flag("service-account-id", "string", true);
const USER_ID: Field = flag("user-id", "string", true);

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! string_field {
    ($input:ty, $field:ident, $flag:ident, $name:literal, $requirement:expr) => {
        string_field!(
            $input,
            $field,
            $flag,
            $name,
            $requirement,
            std::convert::identity
        )
    };
    ($input:ty, $field:ident, $flag:ident, $name:literal, $requirement:expr, $wrap:path) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut $input, value| {
                let serde_json::Value::String(value) = value else {
                    return Err(());
                };
                input.$field = $wrap(value);
                Ok(())
            },
            set_flag: Some(|input: &mut $input, matches| {
                input.$field = $wrap(matches.get_one::<String>($flag.name).ok_or(())?.clone());
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

macro_rules! required_string {
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

macro_rules! int_field {
    ($input:ty, $field:ident, $flag:ident, $name:literal) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut $input, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut $input, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! operation {
    ($input:ident, $defaults:expr, $method:ident, $name:literal, $go:literal,
     $help:literal, $example:expr, $notes:expr, [$($flag:ident => $field:expr),* $(,)?]) => {
        operation!(Read; $input, $defaults, $method, $name, $go, $help, $example, $notes, [$($flag => $field),*])
    };
    ($kind:ident; $input:ident, $defaults:expr, $method:ident, $name:literal, $go:literal,
     $help:literal, $example:expr, $notes:expr, [$($flag:ident => $field:expr),* $(,)?]) => {{
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
            sdk_operation: concat!("iam.", $go),
            short_help: $help,
            example: $example,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::$kind,
            flags: &[$($flag),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &INPUT,
                    |ctx, config, input| Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, std::convert::identity)
                    }),
                ))
            })),
        }
    }};
}

mod service_account_writes;
mod user_writes;

const PAGE_NOTE: &str =
    "Returns one zero-based page. The default size is 10000. Unknown page metadata is null.";
const USER_NOTE: &str = "Use `list-user-policies` and `list-user-groups` with the user ID to see attached policies and memberships.";
const ACCOUNT_NOTE: &str =
    "Use `list-service-account-policies` with the service account ID to see attached policies.";

macro_rules! write_rule {
    ($identity:ident, $rank:expr) => {
        Rule {
            identity: stringify!($identity),
            matches: |error| error.detail::<sdk::WriteError>() == Some(&sdk::WriteError::$identity),
            name: Some(RuleName {
                rank: $rank,
                code: stringify!($identity),
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        }
    };
}

pub(super) const SERVICE: Service = Service {
    name: "iam",
    error_rules: &[
        Rule {
            identity: "IamUserWriteUncertain",
            matches: |error| {
                matches!(
                    error.detail::<sdk::WriteError>(),
                    Some(
                        sdk::WriteError::UserWriteUncertain
                            | sdk::WriteError::UserCreateUncertain
                            | sdk::WriteError::OriginNotIam
                    )
                )
            },
            name: Some(RuleName {
                rank: 19,
                code: "RequestFailed",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        write_rule!(SelfChange, 41),
        write_rule!(PrivilegedChange, 42),
        write_rule!(ResourceInUse, 44),
    ],
    operations: &[
        user_writes::CREATE,
        user_writes::DELETE,
        user_writes::RESET,
        user_writes::ENABLE_MFA,
        user_writes::DISABLE_MFA,
        service_account_writes::CREATE,
        service_account_writes::RESET,
        operation!(
            DestructiveWrite; DeleteServiceAccountInput, || sdk::DeleteServiceAccountInput::new(""), delete_service_account,
            "delete-service-account", "DeleteServiceAccount", "Delete an IAM service account",
            Some("greennode iam delete-service-account --service-account-id <service-account-id> --yes"),
            &["Refuses service-account callers and accounts with an attached IAM write policy."], [
                SERVICE_ACCOUNT_ID => required_string!(sdk::DeleteServiceAccountInput, service_account_id, SERVICE_ACCOUNT_ID, "ServiceAccountID"),
            ]
        ),
        service_account_writes::UPDATE,
        operation!(
            Write; CreateGroupInput, || sdk::CreateGroupInput::new(""), create_group,
            "create-group", "CreateGroup", "Create an empty IAM group",
            Some("greennode iam create-group --name <name>"),
            &["Creates an IAM-mode group with no members or policies. If the outcome is uncertain, list groups before repeating the command."], [
                REQUIRED_NAME => required_string!(sdk::CreateGroupInput, name, REQUIRED_NAME, "Name"),
                DESCRIPTION => string_field!(sdk::CreateGroupInput, description, DESCRIPTION, "Description", Requirement::Optional),
            ]
        ),
        operation!(
            DestructiveWrite; DeleteGroupInput, || sdk::DeleteGroupInput::new(""), delete_group,
            "delete-group", "DeleteGroup", "Delete an empty IAM group",
            Some("greennode iam delete-group --group-id <group-id> --yes"),
            &["Requires a recognized caller type and a verified IAM group with no users or policies. An attachment returns `ResourceInUse`."], [
                GROUP_ID => required_string!(sdk::DeleteGroupInput, group_id, GROUP_ID, "GroupID"),
            ]
        ),
        operation!(
            GetCallerIdentityInput,
            sdk::GetCallerIdentityInput::default,
            get_caller_identity,
            "get-caller-identity",
            "GetCallerIdentity",
            "Read the caller identity",
            None,
            &[],
            []
        ),
        operation!(
            GetGroupInput, || sdk::GetGroupInput::new(""), get_group,
            "get-group", "GetGroup", "Read an IAM group", Some("greennode iam get-group --group-id <group-id> --query Group"), &[], [
                GROUP_ID => required_string!(sdk::GetGroupInput, group_id, GROUP_ID, "GroupID"),
            ]
        ),
        operation!(
            GetPolicyInput, || sdk::GetPolicyInput::new(""), get_policy,
            "get-policy", "GetPolicy", "Read an IAM policy", Some("greennode iam get-policy --policy-id <policy-id> --query Policy"), &[], [
                POLICY_ID => required_string!(sdk::GetPolicyInput, policy_id, POLICY_ID, "PolicyID"),
            ]
        ),
        operation!(
            GetServiceAccountInput, || sdk::GetServiceAccountInput::new(""), get_service_account,
            "get-service-account", "GetServiceAccount", "Read an IAM service account", Some("greennode iam get-service-account --service-account-id <service-account-id> --query ServiceAccount"), &[ACCOUNT_NOTE], [
                SERVICE_ACCOUNT_ID => required_string!(sdk::GetServiceAccountInput, service_account_id, SERVICE_ACCOUNT_ID, "ServiceAccountID"),
            ]
        ),
        operation!(
            GetUserInput, || sdk::GetUserInput::new(""), get_user,
            "get-user", "GetUser", "Read an IAM user", Some("greennode iam get-user --user-id <user-id> --query User"), &["`User.MFA` exposes nullable `Email` and `GoogleAuthenticator` factor states. Contact and enrolment details are withheld.", USER_NOTE], [
                USER_ID => required_string!(sdk::GetUserInput, user_id, USER_ID, "UserID"),
            ]
        ),
        operation!(
            ListActionsInput,
            sdk::ListActionsInput::default,
            list_actions,
            "list-actions",
            "ListActions",
            "List IAM actions",
            None,
            &[],
            []
        ),
        operation!(
            ListGroupPoliciesInput, || sdk::ListGroupPoliciesInput::new(""), list_group_policies,
            "list-group-policies", "ListGroupPolicies", "List policies attached to an IAM group", None, &[PAGE_NOTE], [
                GROUP_ID => required_string!(sdk::ListGroupPoliciesInput, group_id, GROUP_ID, "GroupID"),
                PAGE => int_field!(sdk::ListGroupPoliciesInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListGroupPoliciesInput, size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListGroupsInput,
            sdk::ListGroupsInput::default,
            list_groups,
            "list-groups",
            "ListGroups",
            "List IAM groups",
            None,
            &[],
            []
        ),
        operation!(
            ListPoliciesInput, sdk::ListPoliciesInput::default, list_policies,
            "list-policies", "ListPolicies", "List IAM policies", None, &[PAGE_NOTE], [
                NAME => string_field!(sdk::ListPoliciesInput, name, NAME, "Name", Requirement::Optional),
                PAGE => int_field!(sdk::ListPoliciesInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListPoliciesInput, size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListPolicyAttachmentsInput, || sdk::ListPolicyAttachmentsInput::new(""), list_policy_attachments,
            "list-policy-attachments", "ListPolicyAttachments", "List the principals attached to an IAM policy", None, &[], [
                POLICY_ID => required_string!(sdk::ListPolicyAttachmentsInput, policy_id, POLICY_ID, "PolicyID"),
            ]
        ),
        operation!(
            ListServiceAccountPoliciesInput, || sdk::ListServiceAccountPoliciesInput::new(""), list_service_account_policies,
            "list-service-account-policies", "ListServiceAccountPolicies", "List policies attached to a service account", None, &[PAGE_NOTE], [
                SERVICE_ACCOUNT_ID => required_string!(sdk::ListServiceAccountPoliciesInput, service_account_id, SERVICE_ACCOUNT_ID, "ServiceAccountID"),
                PAGE => int_field!(sdk::ListServiceAccountPoliciesInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListServiceAccountPoliciesInput, size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListServiceAccountsInput, sdk::ListServiceAccountsInput::default, list_service_accounts,
            "list-service-accounts", "ListServiceAccounts", "List IAM service accounts", None, &[PAGE_NOTE, ACCOUNT_NOTE], [
                NAME => string_field!(sdk::ListServiceAccountsInput, name, NAME, "Name", Requirement::Optional),
                PAGE => int_field!(sdk::ListServiceAccountsInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListServiceAccountsInput, size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListUserGroupsInput, || sdk::ListUserGroupsInput::new(""), list_user_groups,
            "list-user-groups", "ListUserGroups", "List the groups containing an IAM user", None, &[], [
                USER_ID => required_string!(sdk::ListUserGroupsInput, user_id, USER_ID, "UserID"),
            ]
        ),
        operation!(
            ListUserPoliciesInput, || sdk::ListUserPoliciesInput::new(""), list_user_policies,
            "list-user-policies", "ListUserPolicies", "List policies attached to an IAM user", None, &[PAGE_NOTE], [
                USER_ID => required_string!(sdk::ListUserPoliciesInput, user_id, USER_ID, "UserID"),
                PAGE => int_field!(sdk::ListUserPoliciesInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListUserPoliciesInput, size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListUsersInput, sdk::ListUsersInput::default, list_users,
            "list-users", "ListUsers", "List IAM users", None, &[PAGE_NOTE, "`--name` filters by a case-insensitive substring, as observed on the accounts API.", USER_NOTE], [
                PAGE => int_field!(sdk::ListUsersInput, page, PAGE, "Page"),
                SIZE => int_field!(sdk::ListUsersInput, size, SIZE, "Size"),
                NAME => string_field!(sdk::ListUsersInput, name, NAME, "Name", Requirement::Optional),
            ]
        ),
        operation!(
            Write; UpdateGroupInput, || sdk::UpdateGroupInput::new(""), update_group,
            "update-group", "UpdateGroup", "Update an IAM group name or description",
            Some("greennode iam update-group --group-id <group-id> --description <description>"),
            &["Reads the current name when `--name` is omitted. Omitting `--description` preserves it. Reads the group again after a successful update."], [
                GROUP_ID => required_string!(sdk::UpdateGroupInput, group_id, GROUP_ID, "GroupID"),
                OPTIONAL_NAME => string_field!(sdk::UpdateGroupInput, name, OPTIONAL_NAME, "Name", Requirement::Optional, Some),
                OPTIONAL_DESCRIPTION => string_field!(sdk::UpdateGroupInput, description, OPTIONAL_DESCRIPTION, "Description", Requirement::Optional, Some),
            ]
        ),
    ],
};
