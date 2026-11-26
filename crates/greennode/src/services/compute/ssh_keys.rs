use std::cell::OnceCell;
use std::path::Path;
use std::sync::Arc;

use greennode_sdk::{Config, RequestContext, Secret};
use serde::Serialize;

use super::server_commands::{execute, server_operation};
use super::*;
use crate::cli::{render, secret_file};

const KEY_NAME: Field = flag("name", "string", true);
const PUBLIC_KEY: Field = flag("public-key", "string", true);
const SECRET_FILE: Field = secret_file::FLAG;

#[derive(Default)]
struct CreateArgs {
    name: String,
    secret_file: String,
    destination: OnceCell<Arc<secret_file::Destination>>,
}

const CREATE_INPUT: InputDescriptor<CreateArgs> = InputDescriptor {
    defaults: CreateArgs::default,
    fields: &[
        required_id!(CreateArgs, name, KEY_NAME, "Name"),
        InputField {
            json_name: "SecretFile",
            flag: Some(SECRET_FILE),
            secret: false,
            nullable: false,
            set_json: |_, _| Err(()),
            set_flag: Some(|input, matches| {
                input.secret_file = matches
                    .get_one::<String>(SECRET_FILE.name)
                    .ok_or(())?
                    .clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_input_source),
    guard: Some(check_destination),
};

fn check_destination(input: &CreateArgs, _: &ArgMatches) -> Result<(), InputError> {
    let destination = secret_file::Destination::check(Path::new(&input.secret_file))?;
    input
        .destination
        .set(Arc::new(destination))
        .map_err(|_| InputError("--secret-file destination was already checked"))
}

pub(super) const CREATE: Operation = Operation {
    name: "create-ssh-key",
    sdk_operation: "compute.CreateSSHKey",
    short_help: "Create ssh key",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[KEY_NAME],
    json_fields: &[],
    extra_flags: &[SECRET_FILE],
    notes: &[
        "Saves the issued private key to a new file with mode 0600. If saving fails, attempts to delete the new key.",
    ],
    handler: Some(Handler::new(
        &CREATE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management(
                invocation,
                operation,
                matches,
                &CREATE_INPUT,
                create_ssh_key,
            ))
        },
    )),
};

#[derive(Serialize)]
struct SavedKey<'a> {
    #[serde(rename = "SSHKey")]
    ssh_key: &'a sdk::SshKey,
    #[serde(rename = "PrivateKey")]
    private_key: &'a Secret,
    #[serde(rename = "SecretFile")]
    secret_file: &'a str,
}

fn create_ssh_key<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a CreateArgs,
) -> dispatch::FutureResult<'a, dispatch::ExecutionResult> {
    Box::pin(async move {
        let destination =
            Arc::clone(input.destination.get().ok_or_else(|| {
                secret_file::failure("--secret-file destination was unavailable")
            })?);
        let client = sdk::Client::new(config);
        let result = client
            .create_ssh_key(
                ctx,
                sdk::CreateSshKeyInput {
                    name: input.name.clone(),
                },
            )
            .await;
        let mut output = match result {
            Ok(output) => output,
            Err(failure) => return dispatch::project_result(Err(failure), std::convert::identity),
        };
        let secret = output.private_key().clone();
        let saved = tokio::task::spawn_blocking(move || destination.write(&secret)).await;
        if !matches!(saved, Ok(Ok(()))) {
            let deleted = output.discard_after_delivery_failure().await;
            let id = output.redact_reflections(&output.ssh_key.id);
            let message = if deleted {
                format!("could not write --secret-file; the new ssh key {id} was deleted")
            } else {
                format!("ssh key {id} exists and could not be deleted; delete it manually")
            };
            let mut error = secret_file::failure("could not write --secret-file");
            error.message = output.redact_reflections(&message).to_owned().into();
            return Err(error.into());
        }
        Ok(Some(render::project(&SavedKey {
            ssh_key: &output.ssh_key,
            private_key: output.private_key(),
            secret_file: output.redact_reflections(&input.secret_file),
        })))
    })
}

execute!(import_ssh_key, sdk::ImportSshKeyInput);
server_operation!(
    IMPORT,
    "import-ssh-key",
    "compute.ImportSSHKey",
    Write,
    "Import ssh key",
    "Uploads one line of SSH public key material. The private key stays with the caller.",
    sdk::ImportSshKeyInput,
    import_ssh_key,
    &[KEY_NAME, PUBLIC_KEY],
    &[],
    &[],
    [
        required_id!(sdk::ImportSshKeyInput, name, KEY_NAME, "Name"),
        required_id!(sdk::ImportSshKeyInput, public_key, PUBLIC_KEY, "PublicKey"),
    ]
);

execute!(delete_ssh_key, sdk::DeleteSshKeyInput);
server_operation!(
    DELETE,
    "delete-ssh-key",
    "compute.DeleteSSHKey",
    DestructiveWrite,
    "Delete ssh key",
    "Deletes the specified SSH key. An already deleted key returns `NotFound`.",
    sdk::DeleteSshKeyInput,
    delete_ssh_key,
    &[SSH_KEY_ID],
    &[],
    &[],
    [required_id!(
        sdk::DeleteSshKeyInput,
        ssh_key_id,
        SSH_KEY_ID,
        "SSHKeyID"
    )]
);
