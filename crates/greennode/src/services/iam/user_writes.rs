use std::{cell::OnceCell, path::Path, sync::Arc};

use clap::{ArgMatches, parser::ValueSource};
use greennode_sdk::iam as sdk;
use greennode_sdk::{Config, ErrorClass, RecoveryAdvice, RequestContext, Secret};
use serde::Serialize;

use super::{USER_ID, check_input_source, flag};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::{password, render, secret_file};

const USERNAME: Field = flag("username", "string", true);
const PASSWORD_LENGTH: Field = flag("password-length", "int", false);
const SECRET_FILE: Field = Field {
    required: false,
    ..secret_file::FLAG
};
const METHOD: Field = flag("method", "string", true);

#[derive(Default)]
struct Input {
    operand: String,
    password: Option<Secret>,
    path: String,
    password_length: i64,
    destination: OnceCell<Arc<secret_file::Destination>>,
}

const SECRET_PATH: InputField<Input> = InputField {
    json_name: "SecretFile",
    flag: Some(SECRET_FILE),
    secret: false,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: Some(|input, flags| {
        input.path = flags.get_one::<String>(SECRET_FILE.name).ok_or(())?.clone();
        Ok(())
    }),
    requirement: Requirement::Optional,
};

const PRIVATE_PASSWORD: InputField<Input> = InputField {
    json_name: "Password",
    flag: None,
    secret: true,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: None,
    requirement: Requirement::Optional,
};

const CREATE_INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        required_string!(Input, operand, USERNAME, "Username"),
        int_field!(Input, password_length, PASSWORD_LENGTH, "PasswordLength"),
        PRIVATE_PASSWORD,
        SECRET_PATH,
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_password_source),
    guard: Some(check_destination),
};

const RESET_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[
        required_string!(Input, operand, USER_ID, "UserID"),
        int_field!(Input, password_length, PASSWORD_LENGTH, "PasswordLength"),
        PRIVATE_PASSWORD,
        SECRET_PATH,
    ],
    ..CREATE_INPUT
};

pub(super) const CREATE: Operation = Operation {
    name: "create-user",
    sdk_operation: "iam.CreateUser",
    short_help: "Create an IAM user",
    kind: Kind::Write,
    flags: &[USERNAME, PASSWORD_LENGTH],
    json_fields: &[],
    extra_flags: &[password::FIELD, SECRET_FILE],
    notes: &[
        "Choose `--password-file <path>` (or `-` for stdin) or `--secret-file <new-path>` for a generated password. The generated length is 8 to 50 characters, default 32. A private destination is required for generation. For an uncertain result, reconcile ownership with `list-users --name` before further writes.",
    ],
    example: Some(
        "greennode iam create-user --username <username> --secret-file <private-directory>/password",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_password(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                create,
                bind_password,
            ))
        },
    )),
};

pub(super) const RESET: Operation = Operation {
    name: "reset-user-password",
    sdk_operation: "iam.ResetUserPassword",
    short_help: "Reset an IAM user's password",
    kind: Kind::DestructiveWrite,
    flags: &[USER_ID, PASSWORD_LENGTH],
    notes: &[
        "Choose `--password-file <path>` (or `-` for stdin) or `--secret-file <new-path>` for a generated password. Refuses self-changes and protected users. A reset is sent once. Coordinate recovery with the user and other administrators if the result is uncertain.",
    ],
    example: Some(
        "greennode iam reset-user-password --user-id <user-id> --secret-file <private-directory>/password --yes",
    ),
    handler: Some(Handler::new(
        &RESET_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_password(
                invocation,
                operation,
                matches,
                &RESET_INPUT,
                reset,
                bind_password,
            ))
        },
    )),
    ..CREATE
};

pub(super) const DELETE: Operation = operation!(
    DestructiveWrite; DeleteUserInput, || sdk::DeleteUserInput::new(""), delete_user,
    "delete-user", "DeleteUser", "Delete an IAM user",
    Some("greennode iam delete-user --user-id <user-id> --yes"),
    &["Refuses self-deletion and requires a verified IAM-origin user."], [
        USER_ID => required_string!(sdk::DeleteUserInput, user_id, USER_ID, "UserID"),
    ]
);

macro_rules! mfa_operation {
    ($input:ident, $method:ident, $name:literal, $op:literal, $help:literal) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: || sdk::$input::new("", ""),
            fields: &[
                required_string!(sdk::$input, user_id, USER_ID, "UserID"),
                required_string!(sdk::$input, method, METHOD, "Method"),
            ],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: None,
        };
        Operation {
            name: $name,
            sdk_operation: $op,
            short_help: $help,
            kind: Kind::DestructiveWrite,
            flags: &[USER_ID, METHOD],
            json_fields: &[],
            extra_flags: &[],
            notes: &["Changes an existing Google authenticator factor and confirms the resulting state. Requires `--method google`. If the factor is already in the requested state, the command returns without sending a write."],
            example: Some(concat!("greennode iam ", $name, " --user-id <user-id> --method google --yes")),
            presence_required_fields: &[],
            inherited_bindings: &[],
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(invocation, operation, matches, &INPUT, |ctx, config, input| Box::pin(async move {
                    let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                    Ok(Some(render::project(&result.map_err(super::service_account_writes::operation_failure)?)))
                })))
            })),
        }
    }};
}

pub(super) const ENABLE_MFA: Operation = mfa_operation!(
    EnableUserMfaInput,
    enable_user_mfa,
    "enable-user-mfa",
    "iam.EnableUserMFA",
    "Enable Google MFA for an IAM user"
);
pub(super) const DISABLE_MFA: Operation = mfa_operation!(
    DisableUserMfaInput,
    disable_user_mfa,
    "disable-user-mfa",
    "iam.DisableUserMFA",
    "Disable Google MFA for an IAM user"
);

fn check_password_source(matches: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    let password = password::path(matches);
    let secret = matches
        .get_one::<String>(SECRET_FILE.name)
        .filter(|path| !path.is_empty());
    if password.is_some() == secret.is_some() {
        return Err(InputError(
            "exactly one of --password-file and --secret-file is required",
        ));
    }
    if let Some(path) = password {
        if matches.value_source(PASSWORD_LENGTH.name) == Some(ValueSource::CommandLine) {
            return Err(InputError("--password-length requires --secret-file"));
        }
        password::check_conflict(path, json)?;
    }
    check_input_source(matches, json)
}

fn check_destination(input: &Input, matches: &ArgMatches) -> Result<(), InputError> {
    if password::path(matches).is_some() {
        return if input.password_length == 0 {
            Ok(())
        } else {
            Err(InputError("PasswordLength requires --secret-file"))
        };
    }
    let destination = secret_file::Destination::check(Path::new(&input.path))?;
    input
        .destination
        .set(Arc::new(destination))
        .map_err(|_| InputError("--secret-file destination was already checked"))
}

fn bind_password(input: &mut Input, password: Secret) {
    input.password = Some(password);
}

#[derive(Serialize)]
struct Created<'a> {
    #[serde(rename = "User")]
    user: &'a sdk::UserDetail,
    #[serde(rename = "Password")]
    password_marker: &'static str,
    #[serde(rename = "CreatedID")]
    created_id: &'a str,
    #[serde(rename = "CandidateID")]
    candidate_id: &'a str,
    #[serde(rename = "SecretFile", skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

#[derive(Serialize)]
struct Reset<'a> {
    #[serde(rename = "Password", skip_serializing_if = "Option::is_none")]
    password_marker: Option<&'static str>,
    #[serde(rename = "SecretFile", skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

async fn persist(input: &Input, secret: &Secret) -> bool {
    let Some(destination) = input.destination.get().cloned() else {
        return false;
    };
    let secret = secret.clone();
    tokio::task::spawn_blocking(move || destination.write(&secret))
        .await
        .is_ok_and(|result| result.is_ok())
}

fn create<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let request = sdk::CreateUserInput {
            username: input.operand.clone(),
            password: input.password.clone(),
            password_length: input.password_length,
        };
        let output = match sdk::Client::new(config).create_user(ctx, request).await {
            Ok(output) => output,
            Err(failed) => {
                let error = failed.error();
                let identity = error.detail::<sdk::WriteError>();
                if !matches!(
                    identity,
                    Some(sdk::WriteError::UserCreateUncertain | sdk::WriteError::CreateUnconfirmed)
                ) {
                    return Err(super::service_account_writes::operation_failure(failed));
                }
                let path = sdk::redact_create_user_failure(&failed, &input.path);
                let state = if let Some(password) = sdk::generated_create_user_password(&failed) {
                    if !persist(input, password).await {
                        let outcome = if identity == Some(&sdk::WriteError::UserCreateUncertain) {
                            "create outcome is uncertain and password ownership is unproven"
                        } else {
                            "create was accepted but not confirmed"
                        };
                        return Err(create_file_failure(outcome, path, failed.partial()));
                    }
                    format!("generated password saved to {path}")
                } else {
                    "password was not saved (supplied-password mode)".into()
                };
                let advice = error
                    .detail::<RecoveryAdvice>()
                    .map_or("IAM user creation is unconfirmed", RecoveryAdvice::message);
                let message = if identity == Some(&sdk::WriteError::UserCreateUncertain) {
                    format!(
                        "{state}; password ownership is unproven; reconcile with list-users --name and other administrators before trusting the password, resetting or deleting any candidate; do not retry: {advice}"
                    )
                } else {
                    format!("{state}: {advice}")
                };
                return Err(
                    super::service_account_writes::local_failure("RequestFailed", message).into(),
                );
            }
        };
        let path = if let Some(password) = output.generated_password() {
            if !persist(input, password).await {
                return Err(create_file_failure(
                    "create was accepted",
                    output.redact_reflections(&input.path),
                    Some(&output),
                ));
            }
            Some(output.redact_reflections(&input.path))
        } else {
            None
        };
        Ok(Some(render::project(&Created {
            user: &output.user,
            password_marker: "[redacted]",
            created_id: &output.created_id,
            candidate_id: &output.candidate_id,
            path,
        })))
    })
}

fn reset<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let request = sdk::ResetUserPasswordInput {
            user_id: input.operand.clone(),
            password: input.password.clone(),
            password_length: input.password_length,
        };
        let output = match sdk::Client::new(config)
            .reset_user_password(ctx, request)
            .await
        {
            Ok(output) => output,
            Err(failed) => {
                let error = failed.error();
                let code = if error.has_identity(ErrorClass::NotSettled) {
                    "NotSettled"
                } else if error.detail::<sdk::WriteError>()
                    == Some(&sdk::WriteError::UserWriteUncertain)
                {
                    "RequestFailed"
                } else {
                    return Err(super::service_account_writes::operation_failure(failed));
                };
                let path = sdk::redact_reset_user_failure(&failed, &input.path);
                let state = if let Some(password) = sdk::generated_reset_user_password(&failed) {
                    if !persist(input, password).await {
                        let outcome = if code == "NotSettled" {
                            "reset was accepted but response handling did not settle"
                        } else {
                            "reset outcome is uncertain; the previous password may be unusable"
                        };
                        return Err(reset_file_failure(
                            outcome,
                            path,
                            sdk::redact_reset_user_id(&failed, &input.operand),
                        ));
                    }
                    format!("generated password saved to {path}")
                } else {
                    "password was not saved (supplied-password mode)".into()
                };
                let advice = error
                    .detail::<RecoveryAdvice>()
                    .map_or("IAM password reset is unconfirmed", RecoveryAdvice::message);
                let message = format!(
                    "{state}; do not retry automatically; coordinate recovery with the user and other administrators: {advice}"
                );
                return Err(super::service_account_writes::local_failure(code, message).into());
            }
        };
        let path = if let Some(password) = output.generated_password() {
            if !persist(input, password).await {
                return Err(reset_file_failure(
                    "reset was accepted",
                    output.redact_reflections(&input.path),
                    output.redact_user_id(&input.operand),
                ));
            }
            Some(output.redact_reflections(&input.path))
        } else {
            None
        };
        Ok(Some(render::project(&Reset {
            password_marker: path.map(|_| "[redacted]"),
            path,
        })))
    })
}

fn create_file_failure(
    outcome: &str,
    path: &str,
    output: Option<&sdk::CreateUserOutput>,
) -> dispatch::ExecutionFailure {
    let (target, id) = match output {
        Some(output) if !output.created_id.is_empty() => (
            format!("user {}", output.created_id),
            output.created_id.as_str(),
        ),
        Some(output) => (format!("username {}", output.user.username), "<id>"),
        None => ("unconfirmed user".to_owned(), "<id>"),
    };
    let message = format!(
        "{outcome}; could not save generated password to {path} for {target}; no cloud cleanup was attempted; establish ownership and current state, then delete manually with greennode iam delete-user --user-id {id} --yes"
    );
    super::service_account_writes::local_failure("SecretFileFailed", message).into()
}

fn reset_file_failure(outcome: &str, path: &str, id: &str) -> dispatch::ExecutionFailure {
    let message = format!(
        "{outcome}; could not save generated password to {path} for user {id}; no further request was sent; coordinate a deliberate new reset with greennode iam reset-user-password --user-id {id} --secret-file <fresh-private-path> --yes"
    );
    super::service_account_writes::local_failure("SecretFileFailed", message).into()
}
