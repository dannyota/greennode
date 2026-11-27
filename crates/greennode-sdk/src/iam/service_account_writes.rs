use std::collections::BTreeMap;
use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value, json, value::RawValue};

use super::principal_guard::{optional_field, require_safe};
use super::wire::{REDACTED, Screen, Wire};
use super::writes::{confirmed_identity, error_reason, guard_failed, settle, write_operation};
use super::{
    Api, Client, OBJECT_CAP, Operation, ServiceAccount, WriteError, id_input, invalid_input,
    read_operation,
};
use crate::wire::product::{
    evidence::{Evidence, SentCredentials},
    options::Options,
    redirect::Retry,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, Secret, WriteDispatch};

const CREATE: &str = "iam.CreateServiceAccount";
const UPDATE: &str = "iam.UpdateServiceAccount";
const DELETE: &str = "iam.DeleteServiceAccount";
const RESET: &str = "iam.ResetServiceAccountSecret";
const ACCOUNTS: &str = "accounts-api/v1/service-accounts";
const ACCOUNT: &str = "accounts-api/v1/service-accounts/{}";

/// Names the new service account and its optional description.
#[derive(Clone, Debug)]
pub struct CreateServiceAccountInput {
    /// Name of the new service account.
    pub name: String,
    /// Description sent with the create request.
    pub description: String,
}

impl CreateServiceAccountInput {
    /// Sets the required name and leaves the description empty.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
        }
    }
}

/// Service-account creation result. A failure partial contains only safe metadata.
#[derive(Debug, Serialize)]
pub struct CreateServiceAccountOutput {
    /// Confirmed service account or safe identity retained from create.
    #[serde(rename = "ServiceAccount")]
    pub service_account: ServiceAccount,
    #[serde(skip)]
    cleanup_id: Option<String>,
    #[serde(rename = "ClientSecret", skip_serializing_if = "Option::is_none")]
    client_secret: Option<Secret>,
    #[serde(skip)]
    evidence: Evidence,
}

impl CreateServiceAccountOutput {
    /// Borrows the issued secret for private delivery after successful creation.
    pub fn client_secret(&self) -> Option<&Secret> {
        self.client_secret.as_ref()
    }

    /// Masks credential reflections in a caller-provided string.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        safe_text(&self.evidence, text)
    }

    /// Builds a guarded delete input only if the create identified the account.
    pub fn cleanup_input(&self) -> Option<DeleteServiceAccountInput> {
        self.cleanup(self.client_secret())
    }

    fn cleanup(&self, issued: Option<&Secret>) -> Option<DeleteServiceAccountInput> {
        let id = self.cleanup_id.as_ref()?;
        let mut evidence = Evidence::new();
        evidence.absorb(self.evidence.credentials());
        if let Some(secret) = issued {
            record_issued(&mut evidence, secret);
        }
        Some(DeleteServiceAccountInput {
            service_account_id: id.clone(),
            cleanup: Some(evidence.credentials().clone()),
        })
    }
}

/// Borrows a credential issued by an unconfirmed create for private delivery.
pub fn issued_service_account_secret(
    failure: &Failure<CreateServiceAccountOutput>,
) -> Option<&Secret> {
    failure.private_delivery()
}

/// Builds a guarded cleanup input from an attributable failed create.
pub fn failed_service_account_cleanup(
    failure: &Failure<CreateServiceAccountOutput>,
) -> Option<DeleteServiceAccountInput> {
    failure
        .partial()?
        .cleanup(issued_service_account_secret(failure))
}

/// Masks credential reflections in text associated with a failed create.
pub fn redact_failed_service_account_create<'a>(
    failure: &Failure<CreateServiceAccountOutput>,
    text: &'a str,
) -> &'a str {
    let Some(partial) = failure.partial() else {
        return REDACTED;
    };
    let mut evidence = Evidence::new();
    evidence.absorb(partial.evidence.credentials());
    if let Some(secret) = issued_service_account_secret(failure) {
        record_issued(&mut evidence, secret);
    }
    safe_text(&evidence, text)
}

/// Updates only the supplied fields of an existing service account.
#[derive(Clone, Debug)]
pub struct UpdateServiceAccountInput {
    /// Service-account resource ID.
    pub service_account_id: String,
    /// Replacement description, including an explicitly empty description.
    pub description: Option<String>,
    /// Replacement access-token lifetime in seconds.
    pub access_token_life_span: Option<i64>,
}

impl UpdateServiceAccountInput {
    /// Sets the required identifier without changing any optional fields.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            service_account_id: id.into(),
            description: None,
            access_token_life_span: None,
        }
    }
}

/// Service-account metadata after the update.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateServiceAccountOutput {
    /// Confirmed account, or its safe identity after an unconfirmed write.
    #[serde(rename = "ServiceAccount")]
    pub service_account: ServiceAccount,
}

/// Identifies the service account to delete.
#[derive(Clone, Debug)]
pub struct DeleteServiceAccountInput {
    /// Service-account resource ID.
    pub service_account_id: String,
    cleanup: Option<SentCredentials>,
}

impl DeleteServiceAccountInput {
    /// Sets the required service account identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            service_account_id: id.into(),
            cleanup: None,
        }
    }
}

/// Acknowledges deletion without resource fields.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteServiceAccountOutput {}

id_input!(
    /// Identifies the account whose secret will be rotated.
    ResetServiceAccountSecretInput,
    /// Service-account resource ID.
    service_account_id
);

/// The one-time secret produced by a successful rotation.
#[derive(Debug, Serialize)]
pub struct ResetServiceAccountSecretOutput {
    #[serde(rename = "ClientSecret")]
    client_secret: Secret,
    #[serde(skip)]
    evidence: Evidence,
}

impl ResetServiceAccountSecretOutput {
    /// Borrows the issued secret for private delivery.
    pub fn client_secret(&self) -> &Secret {
        &self.client_secret
    }

    /// Masks credential reflections in a caller-provided string.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        safe_text(&self.evidence, text)
    }

    /// Masks an account ID that reflects a credential or the newly issued secret.
    pub fn redact_account_id<'a>(&self, id: &'a str) -> &'a str {
        if usable_account_id(id, &self.evidence) && self.evidence.can_retain_partial() {
            id
        } else {
            REDACTED
        }
    }
}

#[derive(Default)]
struct IssuedAccount {
    id: String,
    secret: Option<Secret>,
}

impl Wire for IssuedAccount {
    const SHAPE: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "id",
            shape: Shape::Any,
        },
        ShapeField {
            name: "clientSecret",
            shape: Shape::Any,
        },
    ]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields: BTreeMap<String, &RawValue> =
            serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        Ok(Self {
            id: optional_field(&fields, "id")?.unwrap_or_default(),
            secret: read_issued_secret(&fields)?,
        })
    }
}

#[derive(Default)]
struct IssuedSecret(Option<Secret>);

impl Wire for IssuedSecret {
    const SHAPE: Shape<'static> = Shape::Object(&[ShapeField {
        name: "clientSecret",
        shape: Shape::Any,
    }]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str::<Option<_>>(raw.get())?.unwrap_or_default();
        read_issued_secret(&fields).map(Self)
    }
}

fn read_issued_secret(
    fields: &BTreeMap<String, &RawValue>,
) -> Result<Option<Secret>, serde_json::Error> {
    Ok(optional_field::<String>(fields, "clientSecret")?
        .filter(|value| !value.is_empty())
        .map(Secret::new))
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Creates once and confirms the response's attributable identity.
    pub async fn create_service_account(
        &self,
        ctx: &RequestContext,
        input: CreateServiceAccountInput,
    ) -> Result<CreateServiceAccountOutput, Failure<CreateServiceAccountOutput>> {
        if input.name.is_empty() {
            return Err(guard_failed(invalid_input(format!(
                "{CREATE} requires Name"
            ))));
        }
        let mut evidence =
            self.write_evidence(ctx, CREATE, &[], &[&input.name, &input.description])?;
        let op = Operation {
            cap: OBJECT_CAP,
            allow_empty: false,
            ..secret_operation(CREATE, ACCOUNTS, &[StatusCode::CREATED])
        };
        let IssuedAccount { id, secret } = self.write(ctx, Api::Accounts, &op, &[], Some(json!({"name": input.name, "description": input.description})), &mut evidence).await
            .map_err(|failed| uncertain(failed, &evidence, "iam.CreateServiceAccount: create may have already reached the server; check with list-service-accounts and match exactly; a service account found that way has lost its client secret and should have its secret reset"))?;
        if let Some(secret) = &secret {
            record_issued(&mut evidence, secret);
        }
        let confirmation = if usable_account_id(&id, &evidence) {
            self.confirm_account::<CreateServiceAccountOutput>(ctx, &id, &mut evidence)
                .await
                .ok()
        } else {
            None
        };
        let confirmed = confirmation.is_some();
        let account = confirmation.unwrap_or_else(|| safe_account_id(&id, &evidence));
        let cleanup_id = usable_account_id(&account.id, &evidence).then(|| account.id.clone());
        if secret.is_none() || !confirmed {
            let (detail, message) = match (confirmed, secret.is_some()) {
                (true, false) => (
                    WriteError::NoSecret,
                    "iam.CreateServiceAccount: iam: no client secret returned: create response had no client secret; run reset-service-account-secret to get one",
                ),
                (false, false) => (
                    WriteError::CreateUnconfirmed,
                    "iam.CreateServiceAccount: iam: created but not confirmed by a read; the response held no client secret; reconcile with list-service-accounts",
                ),
                _ => (
                    WriteError::CreateUnconfirmed,
                    "iam.CreateServiceAccount: iam: created but not confirmed by a read",
                ),
            };
            let error = outcome(detail, message, &evidence);
            let partial = evidence
                .can_retain_partial()
                .then_some(CreateServiceAccountOutput {
                    service_account: account,
                    cleanup_id,
                    client_secret: None,
                    evidence: evidence.transport_only(),
                });
            let failed = Failure::new(error, partial, WriteDispatch::MayHaveBeenSent);
            return Err(match secret {
                Some(secret) => failed.with_private_delivery(secret),
                None => failed,
            });
        }
        Ok(CreateServiceAccountOutput {
            service_account: account,
            cleanup_id,
            client_secret: secret,
            evidence,
        })
    }

    /// Updates the account after proving it is not protected, then reads it back.
    pub async fn update_service_account(
        &self,
        ctx: &RequestContext,
        input: UpdateServiceAccountInput,
    ) -> Result<UpdateServiceAccountOutput, Failure<UpdateServiceAccountOutput>> {
        let id = input.service_account_id.as_str();
        let mut body = Map::new();
        if let Some(description) = input.description {
            body.insert("description".into(), Value::from(description));
        }
        if let Some(life_span) = input.access_token_life_span {
            body.insert("accessTokenLifeSpan".into(), Value::from(life_span));
        }
        let body = Value::Object(body);
        let mut evidence = self.write_evidence(ctx, UPDATE, &[("ServiceAccountID", id)], &[])?;
        if evidence.output_reflects(&body) {
            return Err(guard_failed(invalid_input(
                "IAM request operand withheld".into(),
            )));
        }
        self.guard_service_account(ctx, UPDATE, id, &mut evidence)
            .await?;
        let op = Operation {
            no_output: true,
            keep_status: true,
            sensitive: true,
            ..write_operation(
                UPDATE,
                Method::PATCH,
                ACCOUNT,
                Retry::Repeatable,
                &[StatusCode::OK],
            )
        };
        if let Err(failed) = self
            .write::<Value, _>(ctx, Api::Accounts, &op, &[id], Some(body), &mut evidence)
            .await
        {
            if failed.write_dispatch() == WriteDispatch::MayHaveBeenSent
                && failed.error().detail::<StatusCode>() == Some(&StatusCode::OK)
            {
                return Err(settle_update(
                    failed,
                    id,
                    "accepted update response failed",
                    &evidence,
                ));
            }
            return Err(failed);
        }
        let service_account =
            self.confirm_account(ctx, id, &mut evidence)
                .await
                .map_err(|failed| {
                    settle_update(
                        failed,
                        id,
                        "service account update was accepted but confirmation failed",
                        &evidence,
                    )
                })?;
        Ok(UpdateServiceAccountOutput { service_account })
    }

    /// Deletes the account only after proving it is not protected.
    pub async fn delete_service_account(
        &self,
        ctx: &RequestContext,
        input: DeleteServiceAccountInput,
    ) -> Result<DeleteServiceAccountOutput, Failure<DeleteServiceAccountOutput>> {
        let id = input.service_account_id.as_str();
        let now = || self.session.clock().monotonic_now();
        let cleanup_context = input
            .cleanup
            .as_ref()
            .map(|_| ctx.child_with_deadline(now() + Duration::from_secs(10)));
        let ctx = cleanup_context.as_ref().unwrap_or(ctx);
        let mut evidence = self.write_evidence(ctx, DELETE, &[("ServiceAccountID", id)], &[])?;
        if let Some(cleanup) = &input.cleanup {
            evidence.absorb(cleanup);
        }
        if evidence.id_reflects(id) {
            return Err(guard_failed(invalid_input(
                "IAM request operand withheld".into(),
            )));
        }
        self.guard_service_account(ctx, DELETE, id, &mut evidence)
            .await?;
        let op = Operation {
            sensitive: true,
            ..write_operation(
                DELETE,
                Method::DELETE,
                ACCOUNT,
                Retry::Repeatable,
                &[StatusCode::NO_CONTENT],
            )
        };
        self.write::<Value, _>(ctx, Api::Accounts, &op, &[id], None, &mut evidence)
            .await?;
        Ok(DeleteServiceAccountOutput {})
    }

    /// Rotates the secret once after checking caller and target protection.
    pub async fn reset_service_account_secret(
        &self,
        ctx: &RequestContext,
        input: ResetServiceAccountSecretInput,
    ) -> Result<ResetServiceAccountSecretOutput, Failure<ResetServiceAccountSecretOutput>> {
        let id = input.service_account_id.as_str();
        let mut evidence = self.write_evidence(ctx, RESET, &[("ServiceAccountID", id)], &[])?;
        self.guard_service_account(ctx, RESET, id, &mut evidence)
            .await?;
        let op = secret_operation(
            RESET,
            "accounts-api/v1/service-accounts/{}/reset-secret",
            &[StatusCode::OK],
        );
        let IssuedSecret(secret) = self.write(ctx, Api::Accounts, &op, &[id], None, &mut evidence).await
            .map_err(|failed| uncertain(failed, &evidence, "iam.ResetServiceAccountSecret: reset may have reached the server; no read shows whether the secret changed, so treat the previous secret as revoked"))?;
        let client_secret = secret.ok_or_else(|| Failure::new(
            outcome(WriteError::NoSecret, &format!("service account {} secret was probably rotated by this call but not returned; no file was written; run reset-service-account-secret again to get a usable value", safe_text(&evidence, id)), &evidence),
            None,
            WriteDispatch::MayHaveBeenSent,
        ))?;
        record_issued(&mut evidence, &client_secret);
        Ok(ResetServiceAccountSecretOutput {
            client_secret,
            evidence,
        })
    }

    async fn confirm_account<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<ServiceAccount, Failure<O>> {
        let op = Operation {
            sensitive: true,
            reject_reflected_credentials: true,
            ..read_operation("iam.GetServiceAccount", ACCOUNT, OBJECT_CAP, false)
        };
        let account: ServiceAccount = self
            .fetch(
                ctx,
                Api::Accounts,
                &op,
                &[("ServiceAccountID", id)],
                &[],
                evidence,
            )
            .await?;
        confirmed_identity(
            &account.id,
            id,
            "iam.GetServiceAccount: service account identity is unproven",
            evidence,
        )?;
        require_safe(
            &account,
            "iam: iam.GetServiceAccount: unsafe guard response",
            evidence,
        )?;
        Ok(account)
    }
}

fn secret_operation(
    id: &'static str,
    template: &'static str,
    success: &'static [StatusCode],
) -> Operation {
    Operation {
        sensitive: true,
        keep_status: true,
        ..write_operation(id, Method::POST, template, Retry::Once, success)
    }
}

fn record_issued(evidence: &mut Evidence, secret: &Secret) {
    evidence.add_redaction(secret);
    let trimmed = secret.expose_secret().trim();
    if trimmed.len() != secret.expose_secret().len() {
        evidence.add_redaction(&Secret::new(trimmed));
    }
}

fn safe_text<'a>(evidence: &Evidence, text: &'a str) -> &'a str {
    if evidence.can_retain_partial() && !evidence.text_reflects(text) {
        text
    } else {
        REDACTED
    }
}

fn usable_account_id(id: &str, evidence: &Evidence) -> bool {
    Options::DEFAULT.path_ids.allows(id) && !evidence.id_reflects(id)
}

fn safe_account_id(id: &str, evidence: &Evidence) -> ServiceAccount {
    let mut account = ServiceAccount::default();
    if usable_account_id(id, evidence) {
        account.id = id.to_owned();
    }
    account.screen(evidence);
    account
}

fn settle_update(
    failed: Failure<UpdateServiceAccountOutput>,
    id: &str,
    message: &str,
    evidence: &Evidence,
) -> Failure<UpdateServiceAccountOutput> {
    settle(
        failed,
        Some(UpdateServiceAccountOutput {
            service_account: safe_account_id(id, evidence),
        }),
        &format!("{UPDATE}: {message}"),
        evidence,
    )
}

fn outcome(detail: WriteError, message: &str, evidence: &Evidence) -> Error {
    let message = if evidence.can_retain_partial() && !evidence.text_reflects(message) {
        message
    } else {
        "IAM write outcome could not be confirmed"
    };
    Error::new(ErrorClass::Api)
        .with_outer_detail(detail)
        .with_outer_detail(RecoveryAdvice::new(message))
}

fn uncertain<O>(failed: Failure<O>, evidence: &Evidence, advice: &'static str) -> Failure<O> {
    let (error, _, dispatch) = failed.into_parts();
    if dispatch == WriteDispatch::NotSent
        || error
            .api_error()
            .and_then(|api| api.status())
            .is_some_and(|status| (400..500).contains(&status))
    {
        return Failure::new(error, None, dispatch);
    }
    let message = format!("{advice}: {}", error_reason(&error));
    let message = if evidence.can_retain_partial() && !evidence.text_reflects(&message) {
        &message
    } else {
        advice
    };
    let error = error.with_outer_detail(RecoveryAdvice::new(message));
    Failure::new(error, None, dispatch)
}
