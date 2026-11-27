use std::collections::{BTreeMap, HashSet};

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use super::user_guard::USER;
use super::user_password::validate_password;
use super::wire::{ObservedResponse, REDACTED, Screen, Wire};
use super::writes::{error_reason, guard_failed, guard_refusal, settle, write_operation};
use super::{
    Api, Client, LIST_CAP, Operation, Page, User, UserDetail, WriteError, id_input, invalid_input,
    read_operation,
};
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, Secret, WriteDispatch};

const USERS: &str = "accounts-api/v1/iam-users";
const CREATE: &str = "iam.CreateUser";
const DELETE: &str = "iam.DeleteUser";
const RESET: &str = "iam.ResetUserPassword";
const WRITE_CAP: usize = 64 << 10;

/// Creates an IAM user with a supplied or generated password.
#[derive(Clone)]
pub struct CreateUserInput {
    /// Required username without whitespace or controls.
    pub username: String,
    /// Private supplied password. None generates a password.
    pub password: Option<Secret>,
    /// Length for a generated password. Zero uses 32 characters.
    pub password_length: i64,
}

impl CreateUserInput {
    /// Sets the username and selects password generation.
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: None,
            password_length: 0,
        }
    }
}

/// Safe user metadata and any generated password available for private delivery.
#[derive(Serialize)]
pub struct CreateUserOutput {
    /// Confirmed user details or safe partial metadata.
    #[serde(rename = "User")]
    pub user: UserDetail,
    #[serde(rename = "Password")]
    password_marker: &'static str,
    /// An attributable ID from the create response.
    #[serde(rename = "CreatedID")]
    pub created_id: String,
    /// A current exact-name match that does not establish ownership.
    #[serde(rename = "CandidateID")]
    pub candidate_id: String,
    #[serde(skip)]
    password: Option<Secret>,
    #[serde(skip)]
    evidence: Evidence,
}

crate::wire::product::output::public_fields!(custom_debug CreateUserOutput { user, created_id, candidate_id; password_marker, password, evidence });
crate::wire::product::output::debug_record!(CreateUserOutput {
    user,
    password_marker,
    created_id,
    candidate_id,
    password,
    evidence
});

impl CreateUserOutput {
    /// Borrows the locally generated password, if present.
    pub fn generated_password(&self) -> Option<&Secret> {
        self.password.as_ref()
    }

    /// Masks credential reflections in text associated with this result.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        safe_text(&self.evidence, text)
    }
}

/// Borrows a locally generated password from a failed create for private delivery.
pub fn generated_create_user_password(failure: &Failure<CreateUserOutput>) -> Option<&Secret> {
    failure.private_delivery()
}

/// Masks credential reflections in text associated with a failed create.
pub fn redact_create_user_failure<'a>(
    failure: &Failure<CreateUserOutput>,
    text: &'a str,
) -> &'a str {
    let Some(partial) = failure.partial() else {
        return REDACTED;
    };
    redact_delivery_path(&partial.evidence, failure.private_delivery(), text)
}

id_input!(
    /// Identifies a user to delete.
    DeleteUserInput,
    /// User resource ID.
    user_id
);

/// An acknowledged user deletion.
#[derive(Clone, Serialize)]
pub struct DeleteUserOutput {}

crate::wire::product::output::public_fields!(DeleteUserOutput {});

/// Resets a user's password using a supplied or generated password.
#[derive(Clone)]
pub struct ResetUserPasswordInput {
    /// Required IAM user resource ID.
    pub user_id: String,
    /// Private supplied password. None generates a password.
    pub password: Option<Secret>,
    /// Length for a generated password. Zero uses 32 characters.
    pub password_length: i64,
}

impl ResetUserPasswordInput {
    /// Sets the user ID and selects password generation.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            user_id: id.into(),
            password: None,
            password_length: 0,
        }
    }
}

/// An accepted reset, with a locally generated password when applicable.
#[derive(Serialize)]
pub struct ResetUserPasswordOutput {
    #[serde(rename = "Password", skip_serializing_if = "Option::is_none")]
    password_marker: Option<&'static str>,
    #[serde(skip)]
    password: Option<Secret>,
    #[serde(skip)]
    evidence: Evidence,
}

crate::wire::product::output::public_fields!(ResetUserPasswordOutput { ; password_marker, password, evidence });

impl ResetUserPasswordOutput {
    /// Borrows the locally generated password, if present.
    pub fn generated_password(&self) -> Option<&Secret> {
        self.password.as_ref()
    }

    /// Masks credential reflections in text associated with this result.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        safe_text(&self.evidence, text)
    }

    /// Masks a user ID that reflects any credential from this operation.
    pub fn redact_user_id<'a>(&self, id: &'a str) -> &'a str {
        redact_user_id(&self.evidence, id)
    }
}

/// Borrows a locally generated password from a failed reset for private delivery.
pub fn generated_reset_user_password(
    failure: &Failure<ResetUserPasswordOutput>,
) -> Option<&Secret> {
    failure.private_delivery()
}

/// Masks credential reflections in text associated with a failed reset.
pub fn redact_reset_user_failure<'a>(
    failure: &Failure<ResetUserPasswordOutput>,
    text: &'a str,
) -> &'a str {
    let Some(partial) = failure.partial() else {
        return REDACTED;
    };
    redact_delivery_path(&partial.evidence, failure.private_delivery(), text)
}

/// Masks a user ID that reflects credentials from a failed reset.
pub fn redact_reset_user_id<'a>(
    failure: &Failure<ResetUserPasswordOutput>,
    id: &'a str,
) -> &'a str {
    let Some(partial) = failure.partial() else {
        return REDACTED;
    };
    let mut evidence = Evidence::new();
    evidence.absorb(partial.evidence.credentials());
    if let Some(password) = failure.private_delivery() {
        evidence.add_redaction(password);
    }
    redact_user_id(&evidence, id)
}

#[derive(Default)]
enum CreateAck {
    #[default]
    Empty,
    Object(String),
}

impl Wire for CreateAck {
    const SHAPE: Shape<'static> = Shape::Object(&[ShapeField {
        name: "id",
        shape: Shape::Any,
    }]);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let fields: BTreeMap<String, &RawValue> = serde_json::from_str(raw.get())?;
        let id = super::wire::field(&fields, "id")
            .map(|raw| serde_json::from_str::<String>(raw.get()))
            .transpose()?
            .unwrap_or_default();
        Ok(Self::Object(id))
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Sends one create and confirms its attributable response ID. An exact-name candidate alone cannot prove ownership.
    pub async fn create_user(
        &self,
        ctx: &RequestContext,
        input: CreateUserInput,
    ) -> Result<CreateUserOutput, Failure<CreateUserOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.username.is_empty() {
                return Err(guard_failed(invalid_input(format!(
                    "{CREATE} requires Username"
                ))));
            }
            if input.username.len() > 4096 {
                return Err(guard_failed(invalid_input(format!(
                    "{CREATE}: Username must be valid UTF-8 and at most 4096 bytes"
                ))));
            }
            if !valid_username(&input.username) {
                return Err(guard_failed(invalid_input(format!(
                    "{CREATE}: Username must not contain whitespace or controls"
                ))));
            }
            let length = validate_password(CREATE, input.password.as_ref(), input.password_length)?;
            let (password, generated) = self.prepare_password(ctx, input.password, length)?;
            let username = input.username;
            let mut evidence =
                self.write_evidence(ctx, CREATE, &[], &[&username, password.expose_secret()])?;
            if evidence_with_password_reflects(&evidence, &password, &username) {
                return Err(guard_failed(invalid_input(
                    "IAM request operand withheld".into(),
                )));
            }
            let op = user_write(
                CREATE,
                Method::POST,
                USERS,
                Retry::Once,
                &[StatusCode::CREATED],
            );
            let sent = self
                .write_user_password::<CreateAck, _>(
                    ctx,
                    &op,
                    &[],
                    (Some(&username), &password),
                    &mut evidence,
                )
                .await;
            let ack = match sent {
                Ok(ack) => ack,
                Err(failure) if definite_refusal(&failure) => return Err(failure),
                Err(failure) => {
                    return Err(create_failed(
                        Some(failure.into_parts().0),
                        WriteError::UserCreateUncertain,
                        fallback_user(&username, "", "", &evidence),
                        generated.then_some(password),
                        &evidence,
                    ));
                }
            };
            let id = match ack {
                CreateAck::Empty => String::new(),
                CreateAck::Object(id) => id,
            };
            if id.is_empty() {
                let (candidate, cause) = match self
                    .lookup_created_user(ctx, &username, &mut evidence)
                    .await
                {
                    Ok(candidate) => (candidate.unwrap_or_default(), None),
                    Err(failure) => (String::new(), Some(failure.into_parts().0)),
                };
                return Err(create_failed(
                    cause,
                    WriteError::UserCreateUncertain,
                    fallback_user(&username, "", &candidate, &evidence),
                    generated.then_some(password),
                    &evidence,
                ));
            }
            if !safe_user_id(&id, &evidence) {
                return Err(create_failed(
                    None,
                    WriteError::UserCreateUncertain,
                    fallback_user(&username, "", "", &evidence),
                    generated.then_some(password),
                    &evidence,
                ));
            }
            match self
                .confirm_created_user::<CreateUserOutput>(ctx, &id, &username, &mut evidence)
                .await
            {
                Ok(user) => Ok(CreateUserOutput {
                    user,
                    created_id: id,
                    candidate_id: String::new(),
                    password_marker: "[redacted]",
                    password: generated
                        .then_some(password)
                        .filter(crate::wire::product::exit::private_safe),
                    evidence,
                }),
                Err(failure) => Err(create_failed(
                    Some(failure.into_parts().0),
                    WriteError::CreateUnconfirmed,
                    fallback_user(&username, &id, "", &evidence),
                    generated.then_some(password),
                    &evidence,
                )),
            }
        })
    }

    /// Deletes an IAM user after verifying the target and ruling out self-deletion.
    pub async fn delete_user(
        &self,
        ctx: &RequestContext,
        input: DeleteUserInput,
    ) -> Result<DeleteUserOutput, Failure<DeleteUserOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.user_id.as_str();
            let mut evidence = self.write_evidence(ctx, DELETE, &[("UserID", id)], &[])?;
            self.guard_user(ctx, DELETE, id, false, &mut evidence)
                .await?;
            let op = user_write(
                DELETE,
                Method::DELETE,
                USER,
                Retry::Repeatable,
                &[StatusCode::NO_CONTENT],
            );
            self.write::<Value, _>(ctx, Api::Accounts, &op, &[id], None, &mut evidence)
                .await?;
            Ok(DeleteUserOutput {})
        })
    }

    /// Checks target protection, then sends one reset without a confirmation request.
    pub async fn reset_user_password(
        &self,
        ctx: &RequestContext,
        input: ResetUserPasswordInput,
    ) -> Result<ResetUserPasswordOutput, Failure<ResetUserPasswordOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = input.user_id.as_str();
            let length = validate_password(RESET, input.password.as_ref(), input.password_length)?;
            let supplied = input.password.as_ref().map_or("", Secret::expose_secret);
            let mut evidence = self.write_evidence(ctx, RESET, &[("UserID", id)], &[supplied])?;
            if let Some(password) = &input.password {
                evidence.add_operation_password(password);
            }
            self.guard_user(ctx, RESET, id, true, &mut evidence).await?;
            let (password, generated) = self.prepare_password(ctx, input.password, length)?;
            let op = user_write(
                RESET,
                Method::POST,
                "accounts-api/v1/iam-users/{}/reset-password",
                Retry::Once,
                &[StatusCode::NO_CONTENT],
            );
            match self
                .write_user_password::<Value, _>(ctx, &op, &[id], (None, &password), &mut evidence)
                .await
            {
                Ok(_) => Ok(ResetUserPasswordOutput {
                    password_marker: generated.then_some("[redacted]"),
                    password: generated
                        .then_some(password)
                        .filter(crate::wire::product::exit::private_safe),
                    evidence,
                }),
                Err(failure) if definite_refusal(&failure) => Err(failure),
                Err(failure) => {
                    let partial = evidence
                        .can_retain_partial()
                        .then(|| ResetUserPasswordOutput {
                            password_marker: None,
                            password: None,
                            evidence: evidence.without_operation_passwords(),
                        });
                    let failure = if accepted(&failure) {
                        settle(failure, partial, RESET, &evidence)
                    } else {
                        uncertain_write(failure, RESET, partial, &evidence)
                    };
                    Err(retain_password(failure, generated.then_some(password)))
                }
            }
        })
    }

    async fn confirm_created_user<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        username: &str,
        evidence: &mut Evidence,
    ) -> Result<UserDetail, Failure<O>> {
        let user = self.fetch_verified_user(ctx, CREATE, id, evidence).await?;
        if user.username != username || evidence.text_reflects(username) {
            return Err(guard_refusal(
                "iam: iam.CreateUser: user confirmation identity mismatch",
                None,
            ));
        }
        Ok(user)
    }

    async fn lookup_created_user(
        &self,
        ctx: &RequestContext,
        username: &str,
        evidence: &mut Evidence,
    ) -> Result<Option<String>, Failure<()>> {
        let op = Operation {
            sensitive: true,
            reflection: crate::wire::product::operation::Reflection::Refuse,
            ..read_operation(CREATE, USERS, LIST_CAP, false)
        };
        let page: Page<User> = self
            .fetch::<_, ()>(
                ctx,
                Api::Accounts,
                &op,
                &[],
                &[
                    ("name", username),
                    ("pageNumber", "0"),
                    ("pageSize", "1000"),
                ],
                evidence,
            )
            .await?;
        let lookup_failure = |reason| guard_refusal(&format!("iam: {CREATE}: {reason}"), None);
        let count = i64::try_from(page.items.len()).unwrap_or(i64::MAX);
        if page.page_number != Some(0)
            || !page
                .page_size
                .is_some_and(|size| (1..=1000).contains(&size) && count <= size)
            || page.total_items != Some(count)
            || (page.total_pages != Some(1) && !(count == 0 && page.total_pages == Some(0)))
        {
            return Err(lookup_failure("incomplete user lookup page"));
        }
        if count == 0 {
            return Ok(None);
        }
        let (mut ids, mut names) = (HashSet::new(), HashSet::new());
        let mut found = None;
        for user in &page.items {
            if !safe_user_id(&user.id, evidence)
                || !valid_username(&user.username)
                || evidence.text_reflects(&user.username)
                || evidence.text_reflects(&user.created_at)
            {
                return Err(lookup_failure("invalid user lookup row"));
            }
            if !ids.insert(user.id.to_ascii_lowercase()) || !names.insert(&user.username) {
                return Err(lookup_failure("ambiguous user lookup page"));
            }
            if user.username == username {
                found = Some(user.id.as_str());
            }
        }
        let found = found.ok_or_else(|| lookup_failure("no exact user lookup match"))?;
        self.confirm_created_user(ctx, found, username, evidence)
            .await
            .map(|user| Some(user.id))
    }
}

pub(super) fn user_write(
    id: &'static str,
    method: Method,
    route: &'static str,
    retry: Retry,
    success: &'static [StatusCode],
) -> Operation {
    Operation {
        sensitive: true,
        keep_status: true,
        cap: WRITE_CAP,
        ..write_operation(id, method, route, retry, success)
    }
}

pub(super) fn accepted<T>(failure: &Failure<T>) -> bool {
    failure.error().detail::<ObservedResponse>() == Some(&ObservedResponse::NoContent)
}

pub(super) fn definite_refusal<T>(failure: &Failure<T>) -> bool {
    failure.write_dispatch() == WriteDispatch::NotSent
        || failure.error().detail::<ObservedResponse>() == Some(&ObservedResponse::ClientRejection)
}

pub(super) fn uncertain_write<T>(
    failure: Failure<T>,
    op: &str,
    partial: Option<T>,
    evidence: &Evidence,
) -> Failure<T> {
    let (cause, _, dispatch) = failure.into_parts();
    let message = format!(
        "{op}: iam: user write outcome is uncertain: {}",
        error_reason(&cause)
    );
    let message = if evidence.can_retain_partial() && !evidence.text_reflects(&message) {
        &message
    } else {
        "iam: user write outcome is uncertain"
    };
    let error = Error::new(ErrorClass::Api)
        .with_cause(cause)
        .with_outer_detail(WriteError::UserWriteUncertain)
        .with_outer_detail(RecoveryAdvice::new(message));
    Failure::new(error, partial, dispatch)
}

fn valid_username(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 4096
        && !username
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}
fn safe_user_id(id: &str, evidence: &Evidence) -> bool {
    PathIds::Strict.allows(id) && !evidence.id_reflects(id)
}

fn redact_user_id<'a>(evidence: &Evidence, id: &'a str) -> &'a str {
    if evidence.can_retain_partial() && safe_user_id(id, evidence) {
        id
    } else {
        REDACTED
    }
}

fn safe_text<'a>(evidence: &Evidence, text: &'a str) -> &'a str {
    if evidence.can_retain_partial() && !evidence.text_reflects(text) {
        text
    } else {
        REDACTED
    }
}

fn evidence_with_password_reflects(evidence: &Evidence, password: &Secret, username: &str) -> bool {
    let mut check = evidence.transport_only();
    check.add_redaction(password);
    check.text_reflects(username)
}

fn fallback_user(
    username: &str,
    id: &str,
    candidate: &str,
    evidence: &Evidence,
) -> CreateUserOutput {
    let retain_id = |value: &str| {
        if safe_user_id(value, evidence) {
            value.to_owned()
        } else {
            String::new()
        }
    };
    let created_id = retain_id(id);
    let mut user = UserDetail::default();
    user.id.clone_from(&created_id);
    user.username = safe_text(evidence, username).to_owned();
    user.screen(evidence);
    CreateUserOutput {
        user,
        created_id,
        candidate_id: retain_id(candidate),
        password_marker: "[redacted]",
        password: None,
        evidence: evidence.without_operation_passwords(),
    }
}

fn redact_delivery_path<'a>(
    evidence: &Evidence,
    password: Option<&Secret>,
    text: &'a str,
) -> &'a str {
    let mut check = Evidence::new();
    check.absorb(evidence.credentials());
    if let Some(password) = password {
        check.add_redaction(password);
    }
    safe_text(&check, text)
}

fn create_failed(
    cause: Option<Error>,
    detail: WriteError,
    output: CreateUserOutput,
    password: Option<Secret>,
    evidence: &Evidence,
) -> Failure<CreateUserOutput> {
    let advice = if detail == WriteError::CreateUnconfirmed {
        "iam.CreateUser: iam: created but not confirmed by a read"
    } else {
        "iam.CreateUser: password ownership is unproven; establish ownership with other administrators before trusting the password, resetting or deleting a candidate: iam: user create outcome is uncertain"
    };
    let (error, message) = match cause {
        Some(cause) => {
            let message = format!("{advice}: {}", error_reason(&cause));
            (Error::new(ErrorClass::Api).with_cause(cause), message)
        }
        None => (Error::new(ErrorClass::Api), advice.to_owned()),
    };
    let message = if evidence.can_retain_partial() && !evidence.text_reflects(&message) {
        &message
    } else {
        "iam: user create outcome is uncertain"
    };
    let error = error
        .with_outer_detail(detail)
        .with_outer_detail(RecoveryAdvice::new(message));
    retain_password(
        Failure::new(
            error,
            evidence.can_retain_partial().then_some(output),
            WriteDispatch::MayHaveBeenSent,
        ),
        password,
    )
}

fn retain_password<T>(failure: Failure<T>, password: Option<Secret>) -> Failure<T> {
    match password.filter(crate::wire::product::exit::private_safe) {
        Some(password) => failure.with_private_delivery(password),
        None => failure,
    }
}
