use super::reads::{Users, complete_absence, valid_id};
use super::*;
use crate::Secret;
use crate::error::SafeText;
use crate::wire::product::{
    options::AttemptCheck, response::reflects_credential_forms, sender::SentCredentials,
};
use bytes::Bytes;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

const CREATE: &str = "containerregistry.CreateUser";
const DELETE: &str = "containerregistry.DeleteUser";
const FIND: &str = "containerregistry.listUsersExactMatch";
const MAX_USER_LIST_PAGES: i64 = 100;

/// Selects a repository and the service-defined actions to grant on it.
#[derive(Clone)]
pub struct UserPermission {
    /// Repository identifier using ASCII letters, digits, and hyphens.
    pub repository_id: String,
    /// Exact action names from the service's permission list.
    pub actions: Vec<String>,
}

crate::error::debug_record!(UserPermission {
    repository_id,
    actions
});

impl UserPermission {
    /// Sets the repository and its requested actions.
    pub fn new(id: impl Into<String>, actions: Vec<String>) -> Self {
        Self {
            repository_id: id.into(),
            actions,
        }
    }
}

/// Creates a registry user with permissions for at least one repository.
#[derive(Clone)]
pub struct CreateUserInput {
    /// Name that must be absent from the exact-name search.
    pub name: String,
    /// Optional service description.
    pub description: String,
    /// Positive lifetime in days when supplied.
    pub duration_days: Option<i64>,
    /// Repository permissions to grant.
    pub permissions: Vec<UserPermission>,
}

crate::error::debug_record!(CreateUserInput {
    name,
    description,
    duration_days,
    permissions
});

impl CreateUserInput {
    /// Sets the name and permissions without a description or expiry.
    pub fn new(name: impl Into<String>, permissions: Vec<UserPermission>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            duration_days: None,
            permissions,
        }
    }
}

/// A registry user create result carrying the issued one-time secret.
#[derive(Serialize)]
pub struct CreateUserOutput {
    /// Empty when the service response cannot prove ownership of a user identity.
    #[serde(rename = "User")]
    pub user: User,
    #[serde(rename = "SecretKey")]
    secret_key: Secret,
    #[serde(skip)]
    evidence: Evidence,
}

crate::wire::product::output::public_fields!(CreateUserOutput { user; secret_key, evidence });

impl CreateUserOutput {
    /// Borrows the one-time secret for storage in a private destination.
    pub fn secret_key(&self) -> &Secret {
        &self.secret_key
    }

    /// Masks a string if it contains a credential from this operation or the issued secret.
    pub fn redact_reflections<'a>(&self, text: &'a str) -> &'a str {
        if reflects_credential_forms(text, self.evidence.credentials()) {
            "<redacted>"
        } else {
            text
        }
    }
}

/// Borrows the issued key from a failed create for private storage.
/// The key can remain available here when the failure has no public partial output.
pub fn issued_user_secret(failure: &Failure<CreateUserOutput>) -> Option<&Secret> {
    failure
        .partial()
        .map(CreateUserOutput::secret_key)
        .or_else(|| failure.private_delivery())
}

/// Identifies a registry user by UUID for deletion.
#[derive(Clone)]
pub struct DeleteUserInput {
    /// Registry user UUID, distinct from the account's `UserID` field.
    pub user_id: String,
}

crate::error::debug_record!(DeleteUserInput { user_id });

impl DeleteUserInput {
    /// Selects the registry user UUID.
    pub fn new(id: impl Into<String>) -> Self {
        Self { user_id: id.into() }
    }
}

/// An acknowledged user deletion with no resource fields.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeleteUserOutput {}

crate::wire::product::output::public_fields!(DeleteUserOutput {});

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuedSecret {
    #[serde(default, deserialize_with = "null_default")]
    secret_key: String,
    #[serde(skip)]
    field_only: bool,
}

impl Output for IssuedSecret {
    const SCHEMA: Shape<'static> = Shape::Object(&[field("secretKey")]);

    fn decode_response(
        op: &Operation,
        response: &crate::wire::product::sender::Response,
    ) -> Result<Self, Box<Error>> {
        if op.success.contains(&response.status)
            && response.clean_receipt().is_none()
            && let Some(secret) = response.private_issuance()
        {
            return Ok(Self {
                secret_key: secret.expose_secret().to_owned(),
                field_only: true,
            });
        }
        crate::wire::product::operation::decode_response(op, response)
    }
}

impl Output for DeleteUserOutput {
    const SCHEMA: Shape<'static> = Shape::Any;
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Validates permissions and complete name-search results, then sends one create.
    /// A secret-only response returns `UserNotFound` with the key available through [`issued_user_secret`].
    /// An exact-name match does not give authority to delete a user.
    pub async fn create_user(
        &self,
        ctx: &RequestContext,
        input: CreateUserInput,
    ) -> Result<CreateUserOutput, Failure<CreateUserOutput>> {
        crate::wire::product::exit::finish!(async {
            validate_create(&input)?;
            let mut evidence = Evidence::for_write();
            let permissions: Permissions = self
                .request::<_, ()>(
                    ctx,
                    &private_read(CREATE, "v1/user/permissions"),
                    &[],
                    &[],
                    Bytes::new(),
                    &mut evidence,
                )
                .await
                .map_err(before_write)?;
            let (body, ids) = create_body(&input, &permissions.0, &evidence)?;
            let found = self.find_user_name(ctx, &input.name, &mut evidence).await.map_err(|failure| {
                let (error, _, _) = failure.into_parts();
                let Some(api) = error.api_error() else {
                    return not_sent(error);
                };
                let message = format!("containerregistry.CreateUser: could not prove the requested user name is absent: {}", api.go_text());
                not_sent(registry_error(api.operation(), api.status(), api.code(), &message).with_cause(error))
            })?;
            if found {
                return Err(invalid_input(
                    "containerregistry.CreateUser: requested name already exists; check list-users --name",
                ));
            }
            let check_ids = |credentials: &SentCredentials| {
                ids.iter()
                    .any(|id| credentials.id_reflects(id))
                    .then(|| Box::new(Error::new(ErrorClass::InvalidInput)))
            };
            let options = Options {
                attempt_check: Some(AttemptCheck(&check_ids)),
                ..Options::DEFAULT
            };
            let issued: IssuedSecret = self.request_with_options(
                ctx, &Operation { allow_empty: true, sensitive: true, reflection: crate::wire::product::operation::Reflection::PrivateDelivery("secretKey"), ..write_operation(
                    CREATE, Method::POST, "v1/user", &[StatusCode::OK],
                ) }, &[], &[], (Bytes::from(body.to_string()), options), &mut evidence,
            ).await.map_err(|failure| create_failure(failure, concat!(
                "containerregistry.CreateUser: create may have already reached the server; ",
                "check list-users --name and reconcile ownership before any cleanup",
            )))?;
            let secret_key = Secret::new(issued.secret_key);
            if secret_key.expose_secret().is_empty()
                || evidence.text_reflects(secret_key.expose_secret())
            {
                return Err(Failure::new(
                    registry_error(
                        CREATE,
                        Some(200),
                        "",
                        "create response had no secret key; a user may exist, check list-users --name and reconcile ownership before cleanup",
                    ),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
            evidence.add_secret(&secret_key);
            let trimmed_key = secret_key.expose_secret().trim();
            if !trimmed_key.is_empty() && trimmed_key.len() != secret_key.expose_secret().len() {
                evidence.add_secret(&Secret::new(trimmed_key));
            }
            let lookup = if issued.field_only {
                None
            } else {
                Some(self.find_user_name(ctx, &input.name, &mut evidence).await)
            };
            let (message, cause) = match lookup {
                None | Some(Ok(_)) => (
                    "containerregistry: created user not found: containerregistry.CreateUser: a name match cannot prove creation ownership; reconcile with list-users before any cleanup",
                    None,
                ),
                Some(Err(failure)) => (
                    "containerregistry: created user not found: containerregistry.CreateUser: candidate lookup failed; reconcile with list-users before any cleanup",
                    Some(failure.into_parts().0),
                ),
            };
            let error =
                registry_error("", None, "", message).with_detail(ErrorDetail::UserNotFound);
            let error = match cause {
                Some(cause) => error.with_cause(cause),
                None => error,
            };
            let (error, partial, dispatch) = evidence
                .with_partial(
                    failure::<()>(error),
                    Value::Null,
                    WriteDispatch::MayHaveBeenSent,
                )
                .into_parts();
            if !crate::wire::product::exit::private_safe(&secret_key) {
                return Err(Failure::new(error, None, dispatch));
            }
            if partial.is_some() {
                Err(Failure::new(
                    error,
                    Some(CreateUserOutput {
                        user: User::default(),
                        secret_key,
                        evidence,
                    }),
                    dispatch,
                ))
            } else {
                Err(Failure::new(error, None, dispatch).with_private_delivery(secret_key))
            }
        })
    }

    /// Deletes a user once. Confirms absence after an ambiguous server error.
    pub async fn delete_user(
        &self,
        ctx: &RequestContext,
        input: DeleteUserInput,
    ) -> Result<DeleteUserOutput, Failure<DeleteUserOutput>> {
        crate::wire::product::exit::finish!(async {
            let id = &input.user_id;
            if !valid_id(id) {
                return Err(invalid_input(
                    "containerregistry.DeleteUser requires a valid UserID",
                ));
            }
            let mut evidence = Evidence::for_write();
            let result = self
                .request(
                    ctx,
                    &Operation {
                        allow_empty: true,
                        no_output: true,
                        reflection: crate::wire::product::operation::Reflection::Redact,
                        ..write_operation(DELETE, Method::DELETE, "v1/user/{}", &[StatusCode::OK])
                    },
                    &[id],
                    &[],
                    Bytes::new(),
                    &mut evidence,
                )
                .await;
            match result {
                Err(original) if is_server_error(original.error()) => {
                    let result = self
                        .fallback_inventory::<Users, ()>(
                            ctx,
                            "containerregistry.userFoundAfterServerError",
                            "v1/user",
                            &[("name", ""), ("size", "10000")],
                            &mut evidence,
                        )
                        .await;
                    if result.is_ok_and(|receipt| {
                        complete_absence(
                            receipt,
                            &evidence,
                            id,
                            |list| {
                                let page = list.into_page()?;
                                Some((page.items, page.total_page, page.total_item))
                            },
                            |item| &item.id,
                        )
                    }) {
                        return Err(Failure::new(
                            Error::new(ErrorClass::NotFound).with_cause(registry_error(
                                "",
                                None,
                                "",
                                "containerregistry.DeleteUser: user not found",
                            )),
                            None,
                            original.write_dispatch(),
                        ));
                    }
                    Err(redact_failure(original, &evidence))
                }
                other => other,
            }
        })
    }

    async fn find_user_name(
        &self,
        ctx: &RequestContext,
        name: &str,
        evidence: &mut Evidence,
    ) -> Result<bool, Failure<()>> {
        if evidence.credentials().reflects(name.as_bytes()) {
            return Err(name_evidence_error("unsafe lookup identity"));
        }
        let mut seen = HashSet::new();
        let mut totals = None;
        let mut found = false;
        for page in 1..=MAX_USER_LIST_PAGES {
            let value: Users = self
                .request(
                    ctx,
                    &private_read(FIND, "v1/user"),
                    &[],
                    &[
                        ("name", name),
                        ("page", &page.to_string()),
                        ("size", "10000"),
                    ],
                    Bytes::new(),
                    evidence,
                )
                .await?;
            let value = value
                .into_page()
                .ok_or_else(|| name_evidence_error("invalid list metadata"))?;
            if *totals.get_or_insert((value.total_page, value.total_item))
                != (value.total_page, value.total_item)
                || (value.page != 0 && value.page != page)
                || (page < value.total_page && value.items.is_empty())
            {
                return Err(name_evidence_error("unstable or nonprogressing user pages"));
            }
            for user in value.items {
                if !valid_id(&user.id) || !seen.insert(user.id) {
                    return Err(name_evidence_error("missing or repeated user identity"));
                }
                if user.name.trim().is_empty() || user.name.chars().any(char::is_control) {
                    return Err(name_evidence_error("missing or invalid user name"));
                }
                found |= user.name == name;
            }
            let count = i64::try_from(seen.len()).unwrap_or(i64::MAX);
            if count > value.total_item || (page >= value.total_page && count != value.total_item) {
                return Err(name_evidence_error(
                    "user list totals do not match the returned identities",
                ));
            }
            if page >= value.total_page {
                return Ok(found);
            }
        }
        Err(name_evidence_error(
            "exceeded 100 pages without reaching the reported last page",
        ))
    }
}

fn private_read(id: &'static str, template: &'static str) -> Operation {
    Operation {
        cap: 4 << 20,
        sensitive: true,
        ..read_operation(id, template)
    }
}

fn name_evidence_error(message: &str) -> Failure<()> {
    failure(registry_error(
        "",
        None,
        "",
        &format!("containerregistry: {message}"),
    ))
}

#[expect(
    clippy::result_large_err,
    reason = "Validation returns the operation's Failure"
)]
fn validate_create(input: &CreateUserInput) -> Result<(), Failure<CreateUserOutput>> {
    if input.name.is_empty() || input.permissions.is_empty() {
        return Err(invalid_input(
            "containerregistry.CreateUser requires Name and at least one permission",
        ));
    }
    if input.duration_days.is_some_and(|days| days < 1) {
        return Err(invalid_input(
            "containerregistry.CreateUser requires DurationDays to be at least 1",
        ));
    }
    for (index, permission) in input.permissions.iter().enumerate() {
        if !valid_id(&permission.repository_id) || permission.actions.is_empty() {
            return Err(invalid_input(&format!(
                "containerregistry.CreateUser requires Permissions[{index}] to have a valid RepositoryID and at least one action"
            )));
        }
    }
    Ok(())
}

#[expect(
    clippy::result_large_err,
    reason = "Validation returns the operation's Failure"
)]
fn create_body<'a>(
    input: &'a CreateUserInput,
    permissions: &'a [Permission],
    evidence: &Evidence,
) -> Result<(Value, Vec<&'a str>), Failure<CreateUserOutput>> {
    let policies: BTreeMap<_, _> = permissions
        .iter()
        .map(|policy| (&policy.action, &policy.id))
        .collect();
    let mut requests = Vec::new();
    let mut identifiers = Vec::new();
    for (i, permission) in input.permissions.iter().enumerate() {
        let ids: Option<Vec<_>> = permission
            .actions
            .iter()
            .map(|action| policies.get(action))
            .collect();
        let ids = ids.ok_or_else(|| {
            let known = policies
                .keys()
                .map(|action| action.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let prefix =
                format!("containerregistry.CreateUser: Permissions[{i}] names an unknown action");
            let message = SafeText::new(&format!("{prefix}; known actions: {known}"), &[]);
            if reflects_credential_forms(message.as_str(), evidence.credentials()) {
                invalid_input(&format!("{prefix}; known actions withheld"))
            } else {
                invalid_input(message.as_str())
            }
        })?;
        identifiers.push(permission.repository_id.as_str());
        identifiers.extend(ids.iter().map(|id| id.as_str()));
        requests.push(json!({"repoId": permission.repository_id, "policyIdList": ids}));
    }
    let mut body = json!({"name": input.name, "permissionRequestList": requests});
    if !input.description.is_empty() {
        body["description"] = json!(input.description);
    }
    if let Some(days) = input.duration_days {
        body["duration"] = json!(days);
    }
    Ok((body, identifiers))
}
