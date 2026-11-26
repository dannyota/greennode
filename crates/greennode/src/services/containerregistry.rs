use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};
use clap::ArgMatches;
use greennode_sdk::containerregistry::{
    Client, GetRepositoryInput, ListPermissionsInput, ListRepositoriesInput,
    ListRepositoryUsersInput, ListUsersInput,
};
use greennode_sdk::{Config, RequestContext};

const ACCESS_LEVEL: Field = Field {
    name: "access-level",
    go_type: "string",
    required: false,
};
const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: false,
};
const PAGE: Field = Field {
    name: "page",
    go_type: "int",
    required: false,
};
const SIZE: Field = Field {
    name: "size",
    go_type: "int",
    required: false,
};
const REPOSITORY_ID: Field = Field {
    name: "repository-id",
    go_type: "string",
    required: true,
};

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}
#[derive(Default)]
struct Input {
    access_level: String,
    name: String,
    page: i64,
    size: i64,
    repository_id: String,
}
macro_rules! string_field {
    ($json:literal, $flag:ident, $member:ident, $requirement:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut Input, value| {
                input.$member = value.as_str().ok_or(())?.to_owned();
                Ok(())
            },
            set_flag: Some(|input: &mut Input, matches| {
                input.$member = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}
macro_rules! number_field {
    ($json:literal, $flag:ident, $member:ident) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut Input, value| {
                input.$member = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut Input, matches| {
                input.$member = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}
macro_rules! descriptor {
    ($($field:expr),* $(,)?) => { InputDescriptor { defaults: Input::default, fields: &[$($field),*], allow_json: true, allow_query: true, project_binding: None, pre_guard: Some(check_input_source), guard: None } };
}
const REPOSITORIES: InputDescriptor<Input> = descriptor![
    string_field!(
        "AccessLevel",
        ACCESS_LEVEL,
        access_level,
        Requirement::Optional
    ),
    string_field!("Name", NAME, name, Requirement::Optional)
];
const USERS: InputDescriptor<Input> = descriptor![
    string_field!("Name", NAME, name, Requirement::Optional),
    number_field!("Page", PAGE, page),
    number_field!("Size", SIZE, size)
];
const REPOSITORY_USERS: InputDescriptor<Input> = descriptor![
    string_field!(
        "RepositoryID",
        REPOSITORY_ID,
        repository_id,
        Requirement::Nonzero(|input: &Input| !input.repository_id.is_empty())
    ),
    string_field!("Name", NAME, name, Requirement::Optional),
    number_field!("Page", PAGE, page),
    number_field!("Size", SIZE, size)
];
const GET: InputDescriptor<Input> = descriptor![string_field!(
    "RepositoryID",
    REPOSITORY_ID,
    repository_id,
    Requirement::Nonzero(|input: &Input| !input.repository_id.is_empty())
)];
const EMPTY: InputDescriptor<Input> = descriptor![];

macro_rules! operation {
    ($name:literal, $sdk:literal, $help:literal, $flags:expr, $input:ident, $call:ident) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &$input, $call,
                ))
            })),
        }
    };
}
pub(super) const SERVICE: Service = Service {
    name: "containerregistry",
    error_rules: &[],
    operations: &[
        operation!(
            "list-repositories",
            "containerregistry.ListRepositories",
            "List repositories",
            &[ACCESS_LEVEL, NAME],
            REPOSITORIES,
            list_repositories
        ),
        operation!(
            "get-repository",
            "containerregistry.GetRepository",
            "Get a repository",
            &[REPOSITORY_ID],
            GET,
            get_repository
        ),
        operation!(
            "list-users",
            "containerregistry.ListUsers",
            "List users",
            &[NAME, PAGE, SIZE],
            USERS,
            list_users
        ),
        operation!(
            "list-repository-users",
            "containerregistry.ListRepositoryUsers",
            "List repository users",
            &[REPOSITORY_ID, NAME, PAGE, SIZE],
            REPOSITORY_USERS,
            list_repository_users
        ),
        operation!(
            "list-permissions",
            "containerregistry.ListPermissions",
            "List permissions",
            &[],
            EMPTY,
            list_permissions
        ),
    ],
};
fn list_repositories<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_repositories(
                    ctx,
                    ListRepositoriesInput {
                        access_level: input.access_level.clone(),
                        name: input.name.clone(),
                    },
                )
                .await,
            std::convert::identity,
        )
    })
}
fn get_repository<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .get_repository(ctx, GetRepositoryInput::new(&input.repository_id))
                .await,
            std::convert::identity,
        )
    })
}
fn list_users<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_users(
                    ctx,
                    ListUsersInput {
                        name: input.name.clone(),
                        page: input.page,
                        size: input.size,
                    },
                )
                .await,
            std::convert::identity,
        )
    })
}
fn list_repository_users<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_repository_users(
                    ctx,
                    ListRepositoryUsersInput {
                        repository_id: input.repository_id.clone(),
                        name: input.name.clone(),
                        page: input.page,
                        size: input.size,
                    },
                )
                .await,
            std::convert::identity,
        )
    })
}
fn list_permissions<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    _: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .list_permissions(ctx, ListPermissionsInput {})
                .await,
            std::convert::identity,
        )
    })
}
