use std::cell::OnceCell;
use std::path::Path;
use std::sync::Arc;

use clap::ArgMatches;
use greennode_sdk::iam as sdk;
use greennode_sdk::{Config, ErrorClass, Failure, RecoveryAdvice, RequestContext, Secret};
use serde::Serialize;

use super::{
    DESCRIPTION, OPTIONAL_DESCRIPTION, REQUIRED_NAME, SERVICE_ACCOUNT_ID, check_input_source, flag,
};
use crate::cli::dispatch::{
    self, ExecutionFailure, ExecutionResult, FutureResult, LocalError, OperationFailure,
};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::{render, secret_file};

const LIFE_SPAN: Field = flag("access-token-life-span", "*int", false);
const LIFE_SPAN_FIELD: InputField<sdk::UpdateServiceAccountInput> = InputField {
    json_name: "AccessTokenLifeSpan",
    flag: Some(LIFE_SPAN),
    secret: false,
    nullable: true,
    set_json: |input, value| {
        input.access_token_life_span = if value.is_null() {
            None
        } else {
            Some(value.as_i64().ok_or(())?)
        };
        Ok(())
    },
    set_flag: Some(|input, matches| {
        input.access_token_life_span = matches.get_one::<i64>(LIFE_SPAN.name).copied();
        Ok(())
    }),
    requirement: Requirement::Optional,
};

pub(super) const UPDATE: Operation = operation!(
    Write; UpdateServiceAccountInput, || sdk::UpdateServiceAccountInput::new(""), update_service_account,
    "update-service-account", "UpdateServiceAccount", "Update an IAM service account",
    Some("greennode iam update-service-account --service-account-id <service-account-id>"),
    &["Updates only the fields supplied. Confirms the service-account identity with a read after the update."], [
        SERVICE_ACCOUNT_ID => required_string!(sdk::UpdateServiceAccountInput, service_account_id, SERVICE_ACCOUNT_ID, "ServiceAccountID"),
        OPTIONAL_DESCRIPTION => string_field!(sdk::UpdateServiceAccountInput, description, OPTIONAL_DESCRIPTION, "Description", Requirement::Optional, Some),
        LIFE_SPAN => LIFE_SPAN_FIELD,
    ]
);

#[derive(Default)]
struct Input {
    name: String,
    description: String,
    id: String,
    path: String,
    destination: OnceCell<Arc<secret_file::Destination>>,
}

const SECRET_FILE: Field = secret_file::FLAG;
const SECRET_PATH: InputField<Input> = InputField {
    json_name: "SecretFile",
    flag: Some(SECRET_FILE),
    secret: false,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: Some(|input, matches| {
        input.path = matches
            .get_one::<String>(SECRET_FILE.name)
            .ok_or(())?
            .clone();
        Ok(())
    }),
    requirement: Requirement::Optional,
};

const CREATE_INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        required_string!(Input, name, REQUIRED_NAME, "Name"),
        string_field!(
            Input,
            description,
            DESCRIPTION,
            "Description",
            Requirement::Optional
        ),
        SECRET_PATH,
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: Some(check_destination),
};

const RESET_INPUT: InputDescriptor<Input> = InputDescriptor {
    fields: &[
        required_string!(Input, id, SERVICE_ACCOUNT_ID, "ServiceAccountID"),
        SECRET_PATH,
    ],
    ..CREATE_INPUT
};

pub(super) const CREATE: Operation = Operation {
    name: "create-service-account",
    sdk_operation: "iam.CreateServiceAccount",
    short_help: "Create an IAM service account",
    kind: Kind::Write,
    flags: &[REQUIRED_NAME, DESCRIPTION],
    json_fields: &[],
    extra_flags: &[SECRET_FILE],
    notes: &[
        "Creates the account once and saves the issued secret to a new private file. Confirm creation with `list-service-accounts` before another write if the result is uncertain.",
    ],
    example: Some("greennode iam create-service-account --name <name> --secret-file <secret-file>"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                create,
            ))
        },
    )),
};

pub(super) const RESET: Operation = Operation {
    name: "reset-service-account-secret",
    sdk_operation: "iam.ResetServiceAccountSecret",
    short_help: "Rotate an IAM service-account secret",
    kind: Kind::DestructiveWrite,
    flags: &[SERVICE_ACCOUNT_ID],
    notes: &[
        "Rotates the secret once and saves it to a new private file. Treat the previous secret as revoked after an uncertain result.",
    ],
    example: Some(
        "greennode iam reset-service-account-secret --service-account-id <service-account-id> --secret-file <secret-file> --yes",
    ),
    handler: Some(Handler::new(
        &RESET_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &RESET_INPUT,
                reset,
            ))
        },
    )),
    ..CREATE
};

fn check_destination(input: &Input, _: &ArgMatches) -> Result<(), InputError> {
    let destination = secret_file::Destination::check(Path::new(&input.path))?;
    input
        .destination
        .set(Arc::new(destination))
        .map_err(|_| InputError("--secret-file destination was already checked"))
}

#[derive(Serialize)]
struct Created<'a> {
    #[serde(rename = "ServiceAccount")]
    account: &'a sdk::ServiceAccount,
    #[serde(rename = "ClientSecret")]
    secret: &'a Secret,
    #[serde(rename = "SecretFile")]
    path: &'a str,
}

#[derive(Serialize)]
struct Rotated<'a> {
    #[serde(rename = "ClientSecret")]
    secret: &'a Secret,
    #[serde(rename = "SecretFile")]
    path: &'a str,
}

fn create<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let client = sdk::Client::new(config);
        let mut request = sdk::CreateServiceAccountInput::new(&input.name);
        request.description.clone_from(&input.description);
        match client.create_service_account(ctx, request).await {
            Ok(output) => {
                let secret = output
                    .client_secret()
                    .ok_or_else(|| secret_file::failure("create response held no client secret"))?;
                save_created(&client, input, secret, output.cleanup_input()).await?;
                Ok(Some(render::project(&Created {
                    account: &output.service_account,
                    secret,
                    path: output.redact_reflections(&input.path),
                })))
            }
            Err(failed) => {
                if let Some(secret) = sdk::issued_service_account_secret(&failed) {
                    let cleanup = sdk::failed_service_account_cleanup(&failed);
                    let attributable = cleanup.is_some();
                    save_created(&client, input, secret, cleanup).await?;
                    let path = sdk::redact_failed_service_account_create(&failed, &input.path);
                    let message = match failed.partial().filter(|_| attributable) {
                        Some(partial) => format!(
                            "service account {} was created and its secret saved to {path}, but the read to confirm it failed; run list-service-accounts to check it: iam: created but not confirmed by a read",
                            partial.service_account.id
                        ),
                        None => format!(
                            "create identity and ownership are unproven; secret saved to {path}; reconcile with list-service-accounts before any cleanup: iam: created but not confirmed by a read"
                        ),
                    };
                    return Err(local_failure("RequestFailed", message).into());
                }
                if failed.error().detail::<sdk::WriteError>() == Some(&sdk::WriteError::NoSecret) {
                    let id = failed
                        .partial()
                        .map_or("", |partial| partial.service_account.id.as_str());
                    return Err(local_failure("SecretFileFailed", format!("service account {id} was created but the response held no client secret; no file was written; run reset-service-account-secret to get one")).into());
                }
                Err(operation_failure(failed))
            }
        }
    })
}

fn reset<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let request = sdk::ResetServiceAccountSecretInput::new(&input.id);
        let output = match sdk::Client::new(config)
            .reset_service_account_secret(ctx, request)
            .await
        {
            Ok(output) => output,
            Err(failed)
                if failed.error().detail::<sdk::WriteError>()
                    == Some(&sdk::WriteError::NoSecret) =>
            {
                let message = failed.error().detail::<RecoveryAdvice>().map_or(
                    "reset response held no client secret",
                    RecoveryAdvice::message,
                );
                return Err(local_failure("SecretFileFailed", message.to_owned()).into());
            }
            Err(failed) => return Err(operation_failure(failed)),
        };
        let secret = output.client_secret();
        if !persist(input, secret).await {
            let id = output.redact_reflections(&input.id);
            let message = format!(
                "could not write --secret-file: secret file could not be saved; the service account {id} secret was already reset and cannot be recovered here; run reset-service-account-secret again"
            );
            return Err(local_failure("SecretFileFailed", message).into());
        }
        Ok(Some(render::project(&Rotated {
            secret,
            path: output.redact_reflections(&input.path),
        })))
    })
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

async fn save_created(
    client: &sdk::Client,
    input: &Input,
    secret: &Secret,
    cleanup: Option<sdk::DeleteServiceAccountInput>,
) -> Result<(), LocalError> {
    if persist(input, secret).await {
        return Ok(());
    }
    let Some(cleanup) = cleanup else {
        return Err(secret_file::failure(
            "could not write --secret-file; create identity and ownership are unproven; no cleanup was attempted; reconcile with list-service-accounts before any deletion",
        ));
    };
    let (cleanup_ctx, _) = RequestContext::new();
    let removed = match client.delete_service_account(&cleanup_ctx, cleanup).await {
        Ok(_) => true,
        Err(failed) => {
            failed.error().has_identity(ErrorClass::NotFound)
                || failed.error().api_error().and_then(|api| api.status()) == Some(404)
        }
    };
    let message = if removed {
        "could not write --secret-file; the new service account was deleted"
    } else {
        "could not write --secret-file; cleanup could not be confirmed; reconcile ownership with list-service-accounts before any deletion"
    };
    Err(secret_file::failure(message))
}

fn local_failure(code: &'static str, message: String) -> LocalError {
    LocalError {
        code,
        message: message.into(),
        exit: 1,
    }
}

fn operation_failure<T>(failure: Failure<T>) -> ExecutionFailure {
    let (error, _, dispatch) = failure.into_parts();
    OperationFailure {
        error,
        partial: None,
        dispatch,
        query_partial: false,
    }
    .into()
}
