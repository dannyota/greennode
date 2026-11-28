use std::{collections::BTreeMap, time::Duration};

use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};

use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::wire::strict_json;
use crate::{Error, ErrorClass, Failure, RequestContext, Secret, WriteDispatch};

use super::mutation::{
    Mutation, ReplyPolicy, invalid_mutation_response, not_sent, rejected, settlement_error, status,
    with_screened_advice,
};
use super::quotes::{path_id, required};
use super::wire::{api, invalid_input};
use super::{Client, SshKey};

const CREATE: Mutation = Mutation {
    id: "compute.CreateSSHKey",
    template: "v2/{}/sshKeys",
    method: Method::POST,
    retry: Retry::NonIdempotent,
    success: &[StatusCode::CREATED],
    overflow_overrides_status: false,
};
const IMPORT: Mutation = Mutation {
    id: "compute.ImportSSHKey",
    template: "v2/{}/sshKeys/import",
    method: Method::POST,
    retry: Retry::NonIdempotent,
    success: &[StatusCode::CREATED],
    overflow_overrides_status: false,
};
const DELETE: Mutation = Mutation {
    id: "compute.DeleteSSHKey",
    template: "v2/{}/sshKeys/{}",
    method: Method::DELETE,
    retry: Retry::Repeatable,
    success: &[StatusCode::NO_CONTENT],
    overflow_overrides_status: false,
};

/// Names the SSH key GreenNode will generate.
#[derive(Clone, Default)]
pub struct CreateSshKeyInput {
    /// Name of the new key.
    pub name: String,
}

crate::error::debug_record!(CreateSshKeyInput { name });

/// A generated SSH key and the one-time material for private delivery.
#[derive(Serialize)]
pub struct CreateSshKeyOutput {
    /// Public key record.
    #[serde(rename = "SSHKey")]
    pub ssh_key: SshKey,
    #[serde(rename = "PrivateKey")]
    private_key: Secret,
    #[serde(skip)]
    ownership: crate::wire::product::guard::AcknowledgedId,
    #[serde(skip)]
    issuer: Client,
    #[serde(skip)]
    evidence: Evidence,
}

crate::wire::product::output::public_fields!(custom_debug CreateSshKeyOutput { ssh_key; private_key, ownership, issuer, evidence });

crate::error::debug_record!(CreateSshKeyOutput => |self, f| {
    f.debug_struct("CreateSshKeyOutput").finish_non_exhaustive()
});

impl CreateSshKeyOutput {
    /// Borrows the one-time private key for storage in a private destination.
    pub fn private_key(&self) -> &Secret {
        &self.private_key
    }

    /// Masks text that reflects the issued key or a credential from this operation.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        if self.evidence.text_reflects(text) {
            "<redacted>"
        } else {
            text
        }
    }

    /// Attempts to delete the newly issued key after private delivery fails.
    pub async fn discard_after_delivery_failure(&mut self) -> bool {
        let (ctx, _) = RequestContext::new();
        let ctx = ctx.child_with_deadline(
            self.issuer.session.clock().monotonic_now() + Duration::from_secs(10),
        );
        let mut evidence = Evidence::for_write();
        evidence.absorb(self.evidence.credentials());
        let Ok(id) = self.ownership.recheck(&evidence) else {
            return false;
        };
        if evidence.keep_acknowledgement(&self.ownership).is_err() {
            return false;
        }
        let result = self.issuer.delete_key(&ctx, id, &mut evidence).await;
        result.is_ok()
            || result.is_err_and(|failure| {
                let error = failure.error();
                failure.write_dispatch() == WriteDispatch::MayHaveBeenSent
                    && (crate::confirms_absence(error, DELETE.id, "DELETE", DELETE.template)
                        || already_gone(error))
            })
    }
}

/// Names a public SSH key to import.
#[derive(Clone, Default)]
pub struct ImportSshKeyInput {
    /// Name of the imported key.
    pub name: String,
    /// Public key line to upload.
    pub public_key: String,
}

crate::error::debug_record!(ImportSshKeyInput { name, public_key });

/// The public SSH key record returned by an import.
#[derive(Clone, Serialize)]
pub struct ImportSshKeyOutput {
    /// Imported public key record.
    #[serde(rename = "SSHKey")]
    pub ssh_key: SshKey,
}

crate::wire::product::output::public_fields!(ImportSshKeyOutput { ssh_key });

/// Identifies an SSH key to delete.
#[derive(Clone, Default)]
pub struct DeleteSshKeyInput {
    /// Identifier of the key to delete.
    pub ssh_key_id: String,
}

crate::error::debug_record!(DeleteSshKeyInput { ssh_key_id });

/// An acknowledgement of SSH key deletion.
#[derive(Clone, Default, Serialize)]
pub struct DeleteSshKeyOutput {}

crate::wire::product::output::public_fields!(DeleteSshKeyOutput {});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Generates an SSH key pair and returns its private key for private delivery.
    pub async fn create_ssh_key(
        &self,
        ctx: &RequestContext,
        input: CreateSshKeyInput,
    ) -> Result<CreateSshKeyOutput, Failure<CreateSshKeyOutput>> {
        crate::wire::product::exit::finish!(async {
            required(CREATE.id, &[("Name", !input.name.is_empty())])
                .map_err(|error| not_sent(*error))?;
            let mut evidence = Evidence::for_write();
            let body = json!({"name": input.name});
            self.guard_write(ctx, &body, &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let reply = self
                .send_mutation(
                    ctx,
                    &CREATE,
                    &[],
                    Some(&body),
                    ReplyPolicy::PrivateKey,
                    &mut evidence,
                )
                .await
                .map_err(|error| ambiguous(error, &CREATE, &input.name, &evidence))?;
            let receipt = reply.clean_receipt().ok_or_else(|| {
                ambiguous(
                    sent(acknowledgement_error(&reply, &CREATE, &evidence)),
                    &CREATE,
                    &input.name,
                    &evidence,
                )
            })?;
            let (mut ssh_key, private_key) = decode_key(receipt.body(), &CREATE, &evidence)
                .map_err(|error| ambiguous(sent(*error), &CREATE, &input.name, &evidence))?;
            if evidence.text_reflects(private_key.expose_secret()) {
                return Err(sent(invalid_mutation_response(&CREATE, &evidence)));
            }
            evidence.add_redaction(&private_key);
            if !ssh_key.id.is_empty() && !PathIds::Strict.allows(&ssh_key.id) {
                return Err(sent(invalid_mutation_response(&CREATE, &evidence)));
            }
            if !valid_key_output(&mut ssh_key, &evidence) || private_key.expose_secret().is_empty()
            {
                let message = if PathIds::Strict.allows(&ssh_key.id)
                    && !evidence.id_reflects(&ssh_key.id)
                    && private_key.expose_secret().is_empty()
                {
                    format!(
                        "create response for ssh key {} had no private key; its private key is lost, delete that key directly",
                        ssh_key.id
                    )
                } else {
                    "create response had no id or no private key; a key may exist, check with list-ssh-keys".into()
                };
                return Err(sent(api(CREATE.id, Some(201), "", &message, &evidence)));
            }
            if evidence.text_reflects("PrivateKey")
                || evidence.text_reflects("[redacted]")
                || serde_json::to_value(&ssh_key)
                    .map_or(true, |value| evidence.output_reflects(&value))
            {
                return Err(sent(invalid_mutation_response(&CREATE, &evidence)));
            }
            Ok(CreateSshKeyOutput {
                ownership: receipt.bind_id(ssh_key.id.clone()),
                issuer: self.clone(),
                ssh_key,
                private_key,
                evidence,
            })
        })
    }

    /// Imports an SSH public key without sending private key material.
    pub async fn import_ssh_key(
        &self,
        ctx: &RequestContext,
        input: ImportSshKeyInput,
    ) -> Result<ImportSshKeyOutput, Failure<ImportSshKeyOutput>> {
        crate::wire::product::exit::finish!(async {
            required(
                IMPORT.id,
                &[
                    ("Name", !input.name.is_empty()),
                    ("PublicKey", !input.public_key.is_empty()),
                ],
            )
            .map_err(|error| not_sent(*error))?;
            let public_key = input.public_key.trim();
            if public_key.contains(['\n', '\r']) {
                return Err(not_sent(invalid_input(
                    "compute.ImportSSHKey requires PublicKey to be one line",
                )));
            }
            if public_key.contains("PRIVATE KEY") {
                return Err(not_sent(invalid_input(
                    "compute.ImportSSHKey: PublicKey looks like a private key, not a public one",
                )));
            }
            let mut evidence = Evidence::for_write();
            let body = json!({"name": input.name, "pubKey": public_key});
            self.guard_write(ctx, &body, &mut evidence)
                .map_err(|error| not_sent(*error))?;
            let reply = self
                .send_mutation(
                    ctx,
                    &IMPORT,
                    &[],
                    Some(&body),
                    ReplyPolicy::SshPublic,
                    &mut evidence,
                )
                .await
                .map_err(|error| ambiguous(error, &IMPORT, &input.name, &evidence))?;
            let receipt = reply.clean_receipt().ok_or_else(|| {
                ambiguous(
                    sent(acknowledgement_error(&reply, &IMPORT, &evidence)),
                    &IMPORT,
                    &input.name,
                    &evidence,
                )
            })?;
            let (mut ssh_key, unexpected_private_key) =
                decode_key(receipt.body(), &IMPORT, &evidence)
                    .map_err(|error| ambiguous(sent(*error), &IMPORT, &input.name, &evidence))?;
            evidence.add_redaction(&unexpected_private_key);
            if ssh_key.id.is_empty() {
                return Err(sent(api(
                    IMPORT.id,
                    Some(201),
                    "",
                    "import response had no id",
                    &evidence,
                )));
            }
            if !valid_key_output(&mut ssh_key, &evidence) {
                return Err(sent(invalid_mutation_response(&IMPORT, &evidence)));
            }
            Ok(ImportSshKeyOutput { ssh_key })
        })
    }

    /// Deletes an SSH key and recognizes the provider's already-gone response.
    pub async fn delete_ssh_key(
        &self,
        ctx: &RequestContext,
        input: DeleteSshKeyInput,
    ) -> Result<DeleteSshKeyOutput, Failure<DeleteSshKeyOutput>> {
        crate::wire::product::exit::finish!(async {
            self.delete_key(ctx, &input.ssh_key_id, &mut Evidence::for_write())
                .await
        })
    }

    async fn delete_key(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<DeleteSshKeyOutput, Failure<DeleteSshKeyOutput>> {
        path_id(DELETE.id, "SSHKeyID", id).map_err(|error| not_sent(*error))?;
        self.guard_write(ctx, &json!(id), evidence)
            .map_err(|error| not_sent(*error))?;
        self.send_mutation::<()>(ctx, &DELETE, &[id], None, ReplyPolicy::SshPublic, evidence)
            .await
            .map(drop)
            .map_err(|failure| {
                let (error, _, dispatch) = failure.into_parts();
                let error = if !rejected(&error)
                    && status(&error).is_some_and(|status| (400..500).contains(&status))
                {
                    settlement_error(
                        DELETE.id,
                        error,
                        "read get-ssh-key to confirm the deletion",
                        evidence,
                    )
                } else if already_gone(&error) {
                    Error::new(ErrorClass::NotFound).with_cause(error)
                } else {
                    error
                };
                Failure::new(error, None, dispatch)
            })?;
        Ok(DeleteSshKeyOutput {})
    }
}

fn valid_key_output(key: &mut SshKey, evidence: &Evidence) -> bool {
    evidence.can_retain_partial()
        && PathIds::WithUnderscore.allows(&key.id)
        && !evidence.id_reflects(&key.id)
        && [&key.name, &key.public_key, &key.created_at, &key.status]
            .into_iter()
            .all(|value| !value.contains("PRIVATE KEY-----"))
        && key.redact(evidence)
        && !evidence.text_reflects("SSHKey")
}

struct KeyAlreadyGone(crate::wire::product::absence::CleanRequest);

crate::error::debug_record!(KeyAlreadyGone unit);

fn already_gone(error: &Error) -> bool {
    error
        .internal_detail::<KeyAlreadyGone>()
        .is_some_and(|proof| proof.0.matches(DELETE.id, "DELETE", DELETE.template))
}

pub(super) fn key_already_gone(
    receipt: &crate::wire::product::sender::Response,
    requested: &[&str],
    mut error: Error,
) -> Error {
    if let [id] = requested
        && receipt.status == StatusCode::BAD_REQUEST
        && let Some(request) = receipt.clean_request()
        && let Some(body) = receipt.clean_receipt()
        && request.matches(DELETE.id, "DELETE", DELETE.template)
        && crate::wire::product::response::error_message_matches(
            body.body(),
            &receipt.credentials,
            |message| provider_key_absent(message, id),
        )
    {
        error = error.with_outer_detail(KeyAlreadyGone(request));
    }
    error
}

fn provider_key_absent(message: &str, id: &str) -> bool {
    message
        .to_ascii_lowercase()
        .split_once("cannot get ssh key with id")
        .is_some_and(|(_, tail)| {
            tail.trim_start_matches(|c: char| {
                c.is_ascii_whitespace() || matches!(c, ':' | '"' | '\'')
            })
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            .next()
            .is_some_and(|value| value.eq_ignore_ascii_case(id))
        })
}

fn acknowledgement_error(
    reply: &crate::wire::product::sender::Response,
    mutation: &Mutation,
    evidence: &Evidence,
) -> Error {
    if reply.malformed_json() {
        api(
            mutation.id,
            None,
            "",
            "response failed to decode; body withheld",
            evidence,
        )
        .with_outer_detail(crate::wire::product::operation::DecodeFailure)
    } else {
        invalid_mutation_response(mutation, evidence)
    }
}

fn decode_key(
    body: &[u8],
    mutation: &Mutation,
    evidence: &Evidence,
) -> Result<(SshKey, Secret), Box<Error>> {
    #[derive(Deserialize)]
    struct Envelope<'a> {
        #[serde(
            borrow,
            default,
            deserialize_with = "crate::wire::product::lossy::optional_borrowed"
        )]
        data: Option<&'a RawValue>,
    }

    static FIELDS: &[strict_json::Field<'_>] = &[
        strict_json::Field {
            name: "id",
            children: &[],
        },
        strict_json::Field {
            name: "name",
            children: &[],
        },
        strict_json::Field {
            name: "pubKey",
            children: &[],
        },
        strict_json::Field {
            name: "privateKey",
            children: &[],
        },
        strict_json::Field {
            name: "status",
            children: &[],
        },
        strict_json::Field {
            name: "createdAt",
            children: &[],
        },
    ];
    let invalid = || Box::new(invalid_mutation_response(mutation, evidence));
    let text = zeroize::Zeroizing::new(go_json(body).into_owned());
    serde_json::from_str::<&RawValue>(&text).map_err(|_| {
        Box::new(api(
            mutation.id,
            None,
            "",
            "response failed to decode; body withheld",
            evidence,
        ))
    })?;
    strict_json::validate(
        text.as_bytes(),
        &[strict_json::Field {
            name: "data",
            children: FIELDS,
        }],
        8 << 20,
    )
    .map_err(|_| invalid())?;
    if !text
        .trim_start_matches([' ', '\r', '\n', '\t'])
        .starts_with('{')
    {
        return Err(invalid());
    }
    if mutation.id == CREATE.id && !evidence.go_peer_body_screen(body).is_clean() {
        return Err(invalid());
    }
    let envelope: Envelope<'_> = serde_json::from_str(&text).map_err(|_| invalid())?;
    let Some(data) = envelope.data else {
        return Ok((SshKey::default(), Secret::new("")));
    };
    let mut fields: BTreeMap<String, &RawValue> =
        serde_json::from_str(data.get()).map_err(|_| invalid())?;
    if fields
        .keys()
        .any(|key| key != "privateKey" && strict_json::field_matches("privateKey", key))
    {
        return Err(invalid());
    }
    let private_key = if let Some(raw) = fields.remove("privateKey") {
        Secret::new(
            serde_json::from_str::<Field<String>>(raw.get())
                .map_err(|_| invalid())?
                .0,
        )
    } else {
        Secret::new("")
    };
    let key = serde_json::from_str::<Field<SshKey>>(data.get())
        .map_err(|_| invalid())?
        .0;
    Ok((key, private_key))
}

fn ambiguous<O>(
    failure: Failure<O>,
    mutation: &Mutation,
    name: &str,
    evidence: &Evidence,
) -> Failure<O> {
    let (error, _, dispatch) = failure.into_parts();
    let client_response = status(&error).is_some_and(|status| (400..500).contains(&status));
    if dispatch == WriteDispatch::NotSent || rejected(&error) {
        return Failure::new(error, None, dispatch);
    }
    let reason = error
        .api_error()
        .map_or_else(String::new, crate::ApiError::go_text);
    let cleanup = if mutation.id == CREATE.id {
        "; a key found that way has lost its private key and should be deleted"
    } else {
        ""
    };
    let name = if evidence.text_reflects(name) {
        "<redacted>"
    } else {
        name
    };
    let advice = format!(
        "{}: create may have already reached the server; check with list-ssh-keys --name {name:?}, then match exactly{cleanup}: {reason}",
        mutation.id
    );
    let error = if client_response {
        Error::new(ErrorClass::NotSettled).with_cause(error)
    } else {
        error
    };
    Failure::new(
        with_screened_advice(error, &advice, evidence),
        None,
        dispatch,
    )
}

fn sent<O>(error: Error) -> Failure<O> {
    Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
}

pub(super) fn withhold_key_error(body: &[u8]) -> bool {
    let fields = [
        "code",
        "message",
        "error",
        "detail",
        "title",
        "errors",
        "data",
        "privateKey",
    ]
    .map(|name| strict_json::Field {
        name,
        children: &[],
    });
    let text = zeroize::Zeroizing::new(go_json(body).into_owned());
    if strict_json::validate(text.as_bytes(), &fields, 8 << 20).is_err() {
        return true;
    }
    serde_json::from_str::<&RawValue>(&text)
        .and_then(|raw| {
            matches_raw_tokens(
                raw,
                &|name| strict_json::field_matches("privateKey", name),
                &|text| text.contains("PRIVATE KEY-----"),
            )
        })
        .unwrap_or(true)
}

fn matches_raw_tokens(
    raw: &RawValue,
    key_matches: &impl Fn(&str) -> bool,
    value_matches: &impl Fn(&str) -> bool,
) -> Result<bool, serde_json::Error> {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => {
            for (name, item) in serde_json::from_str::<BTreeMap<String, &RawValue>>(text)? {
                if key_matches(&name) || matches_raw_tokens(item, key_matches, value_matches)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Some(b'[') => {
            for item in serde_json::from_str::<Vec<&RawValue>>(text)? {
                if matches_raw_tokens(item, key_matches, value_matches)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Some(b'"') => {
            let value = zeroize::Zeroizing::new(serde_json::from_str::<String>(text)?);
            Ok(value_matches(&value))
        }
        _ => Ok(value_matches(text)),
    }
}
