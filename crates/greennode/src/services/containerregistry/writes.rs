use super::check_input_source;
use crate::cli::dispatch::{self, ExecutionResult, FutureResult, OperationFailure};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::{render, secret_file};
use clap::ArgMatches;
use greennode_sdk::containerregistry::{
    Client, CreateRepositoryInput, CreateUserInput, CreateUserOutput, DeleteRepositoryInput,
    DeleteUserInput, ErrorDetail, User, UserPermission, issued_user_secret,
};
use greennode_sdk::{Config, RequestContext, Secret};
use serde::Serialize;
use serde_json::Value;
use std::{cell::OnceCell, path::Path};

const NAME: Field = Field {
    required: true,
    ..super::NAME
};
const REPOSITORY_ID: Field = super::REPOSITORY_ID;
const USER_ID: Field = Field {
    name: "user-id",
    ..REPOSITORY_ID
};
const NO_WAIT: Field = Field {
    name: "no-wait",
    go_type: "bool",
    required: false,
};
const QUOTA: Field = Field {
    name: "quota-limit-gb",
    go_type: "int",
    required: true,
};
const DESCRIPTION: Field = Field {
    name: "description",
    go_type: "string",
    required: false,
};
const DURATION: Field = Field {
    name: "duration-days",
    go_type: "*int",
    required: false,
};
const PERMISSIONS: Field = Field {
    name: "Permissions",
    go_type: "[]containerregistry.UserPermission",
    required: true,
};
const SECRET_FILE: Field = secret_file::FLAG;
const SAVE_FAILED: &str = "could not write --secret-file; create ownership is unproven and no cleanup was attempted; reconcile ownership with list-users and other administrators before any deletion";

pub(super) const ERROR_RULES: &[Rule] = &[
    rule("RepositoryNotEmpty", 27, RuleExit::Failure, |error| {
        error.detail::<ErrorDetail>() == Some(&ErrorDetail::RepositoryNotEmpty)
    }),
    rule("UserNotFound", 31, RuleExit::Failure, |error| {
        error.detail::<ErrorDetail>() == Some(&ErrorDetail::UserNotFound)
    }),
    rule("InvalidUsage", 49, RuleExit::Usage, |error| {
        error.detail::<ErrorDetail>() == Some(&ErrorDetail::InvalidInput)
    }),
];

const fn rule(
    code: &'static str,
    rank: u16,
    exit: RuleExit,
    matches: fn(&greennode_sdk::Error) -> bool,
) -> Rule {
    Rule {
        identity: code,
        matches,
        name: Some(RuleName {
            rank,
            code,
            operation: RuleOperation::Omit,
            include_api_status: false,
        }),
        exit: Some(exit),
        message: None,
    }
}

#[derive(Default)]
struct Input {
    name: String,
    quota: i64,
    no_wait: bool,
    id: String,
    description: String,
    duration: Option<i64>,
    permissions: Vec<UserPermission>,
    secret_file: String,
    secret_destination: OnceCell<secret_file::Destination>,
}

macro_rules! string_field {
    ($name:literal, $flag:ident, $member:ident, $requirement:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$member = value.as_str().ok_or(())?.to_owned();
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$member = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

const NAME_FIELD: InputField<Input> = string_field!(
    "Name",
    NAME,
    name,
    Requirement::Nonzero(|input| !input.name.is_empty())
);
const NO_WAIT_FIELD: InputField<Input> = InputField {
    json_name: "NoWait",
    flag: Some(NO_WAIT),
    secret: false,
    nullable: false,
    set_json: |input, value| {
        input.no_wait = value.as_bool().ok_or(())?;
        Ok(())
    },
    set_flag: Some(|input, flags| {
        input.no_wait = *flags.get_one::<bool>(NO_WAIT.name).ok_or(())?;
        Ok(())
    }),
    requirement: Requirement::Optional,
};
const CREATE_REPOSITORY_INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        NAME_FIELD,
        InputField {
            json_name: "QuotaLimitGB",
            flag: Some(QUOTA),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.quota = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, flags| {
                input.quota = *flags.get_one::<i64>(QUOTA.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| input.quota != 0),
        },
        NO_WAIT_FIELD,
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: None,
};
const DELETE_REPOSITORY_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[
        string_field!(
            "RepositoryID",
            REPOSITORY_ID,
            id,
            Requirement::Nonzero(|input| !input.id.is_empty())
        ),
        NO_WAIT_FIELD,
    ],
    ..CREATE_REPOSITORY_INPUT
};
const DELETE_USER_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[string_field!(
        "UserID",
        USER_ID,
        id,
        Requirement::Nonzero(|input| !input.id.is_empty())
    )],
    ..CREATE_REPOSITORY_INPUT
};
const CREATE_USER_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[
        NAME_FIELD,
        string_field!(
            "Description",
            DESCRIPTION,
            description,
            Requirement::Optional
        ),
        InputField {
            json_name: "DurationDays",
            flag: Some(DURATION),
            secret: false,
            nullable: true,
            set_json: |input, value| {
                input.duration = if value.is_null() {
                    None
                } else {
                    Some(value.as_i64().ok_or(())?)
                };
                Ok(())
            },
            set_flag: Some(|input, flags| {
                input.duration = flags.get_one::<i64>(DURATION.name).copied();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "Permissions",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.permissions = value
                    .as_array()
                    .ok_or(())?
                    .iter()
                    .map(permission)
                    .collect::<Result<_, _>>()?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Nonzero(|input| !input.permissions.is_empty()),
        },
        InputField {
            json_name: "SecretFile",
            flag: Some(SECRET_FILE),
            secret: false,
            nullable: true,
            set_json: |_, _| Err(()),
            set_flag: Some(|input, flags| {
                input.secret_file = flags.get_one::<String>(SECRET_FILE.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    guard: Some(check_secret_file),
    ..CREATE_REPOSITORY_INPUT
};

fn permission(value: &Value) -> Result<UserPermission, ()> {
    let mut permission = UserPermission::new("", Vec::new());
    for (key, value) in value.as_object().ok_or(())? {
        if key.eq_ignore_ascii_case("RepositoryID") {
            if !value.is_null() {
                permission.repository_id = value.as_str().ok_or(())?.to_owned();
            }
        } else if key.eq_ignore_ascii_case("Actions") {
            permission.actions = if value.is_null() {
                Vec::new()
            } else {
                value
                    .as_array()
                    .ok_or(())?
                    .iter()
                    .map(|action| action.as_str().map(str::to_owned).ok_or(()))
                    .collect::<Result<_, _>>()?
            };
        } else {
            return Err(());
        }
    }
    Ok(permission)
}

fn check_secret_file(input: &Input, _: &ArgMatches) -> Result<(), InputError> {
    let destination = secret_file::Destination::check(Path::new(&input.secret_file))?;
    input
        .secret_destination
        .set(destination)
        .map_err(|_| InputError("--secret-file destination was already checked"))
}

macro_rules! write {
    ($name:literal, $sdk:literal, $help:literal, $kind:ident, $flags:expr, $json:expr,
     $extra:expr, $notes:expr, $example:expr, $input:ident, $call:ident) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            kind: Kind::$kind,
            flags: $flags,
            json_fields: $json,
            extra_flags: $extra,
            notes: &$notes,
            example: $example,
            presence_required_fields: &[],
            inherited_bindings: &[],
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &$input, $call,
                ))
            })),
        }
    };
}

pub(super) const CREATE_REPOSITORY: Operation = write!(
    "create-repository", "containerregistry.CreateRepository", "Create a private repository", Write,
    &[NAME, QUOTA, NO_WAIT], &[], &[],
    ["Creates a private repository and sends the write once. If the outcome is uncertain, use 
       `list-repositories --name <name>` and match the exact name before creating again. Unless
       `--no-wait` is set, confirms the repository by reading it for up to 60 seconds. A `NotSettled`
       error can carry the repository returned by create."],
    None, CREATE_REPOSITORY_INPUT, create_repository
);
pub(super) const DELETE_REPOSITORY: Operation = write!(
    "delete-repository", "containerregistry.DeleteRepository", "Delete an empty repository", DestructiveWrite,
    &[REPOSITORY_ID, NO_WAIT], &[], &[],
    ["Reads the repository first and refuses the delete unless the read proves it contains zero images.
       Unless `--no-wait` is set, waits up to 60 seconds for the repository to become unreadable."],
    None, DELETE_REPOSITORY_INPUT, delete_repository
);
pub(super) const CREATE_USER: Operation = write!(
    "create-user", "containerregistry.CreateUser", "Create a repository user", Write,
    &[NAME, DESCRIPTION, DURATION], &[PERMISSIONS], &[SECRET_FILE],
    ["Use action names from `list-permissions`. The SDK confirms name absence before sending the
       create once. The response supplies a secret without a proven user ID. The CLI saves the secret
       to a new file with Unix mode `0600`, prints the file path, and exits 1 with `UserNotFound`.
       Reconcile ownership with `list-users --name` and other administrators before any deletion or
       another create."],
    Some("greennode containerregistry create-user --name <name> --secret-file <secret-file> --cli-input-json '{\"Permissions\":[{\"RepositoryID\":\"<repository-id>\",\"Actions\":[\"Pull Images\"]}]}'"),
    CREATE_USER_INPUT, create_user
);
pub(super) const DELETE_USER: Operation = write!(
    "delete-user",
    "containerregistry.DeleteUser",
    "Delete a repository user",
    DestructiveWrite,
    &[USER_ID],
    &[],
    &[],
    [
        "The user ID is the registry user's `ID`. A repeat delete of a removed user returns `NotFound` when absence is confirmed."
    ],
    None,
    DELETE_USER_INPUT,
    delete_user
);

fn create_repository<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let mut staged = CreateRepositoryInput::new(&input.name, input.quota);
        staged.no_wait = input.no_wait;
        dispatch::project_result(
            Client::new(config).create_repository(ctx, staged).await,
            std::convert::identity,
        )
    })
}

fn delete_repository<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let mut staged = DeleteRepositoryInput::new(&input.id);
        staged.no_wait = input.no_wait;
        dispatch::project_result(
            Client::new(config).delete_repository(ctx, staged).await,
            std::convert::identity,
        )
    })
}

fn delete_user<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let staged = DeleteUserInput::new(&input.id);
        dispatch::project_result(
            Client::new(config).delete_user(ctx, staged).await,
            std::convert::identity,
        )
    })
}

#[derive(Serialize)]
struct SavedUser<'a> {
    #[serde(rename = "User")]
    user: &'a User,
    #[serde(rename = "SecretKey")]
    secret_key: &'a Secret,
    #[serde(rename = "SecretFile")]
    path: &'a str,
}

fn create_user<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let destination = input
            .secret_destination
            .get()
            .ok_or_else(|| secret_file::failure(SAVE_FAILED))?;
        let mut staged = CreateUserInput::new(&input.name, input.permissions.clone());
        staged.description.clone_from(&input.description);
        staged.duration_days = input.duration;
        let result = Client::new(config).create_user(ctx, staged).await;
        let project = |output: &CreateUserOutput| {
            render::project(&SavedUser {
                user: &output.user,
                secret_key: output.secret_key(),
                path: output.redact_reflections(&input.secret_file),
            })
        };
        let save = |secret: &Secret| {
            destination
                .write(secret)
                .map_err(|_| secret_file::failure(SAVE_FAILED))
        };
        match result {
            Ok(output) => {
                save(output.secret_key())?;
                Ok(Some(project(&output)))
            }
            Err(failure) => {
                if let Some(secret) = issued_user_secret(&failure) {
                    save(secret)?;
                }
                let (error, partial, dispatch) = failure.into_parts();
                let partial = partial.as_ref().and_then(|output| project(output).ok());
                Err(OperationFailure {
                    error,
                    partial,
                    dispatch,
                    query_partial: false,
                }
                .into())
            }
        }
    })
}
