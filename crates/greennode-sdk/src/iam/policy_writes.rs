use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};

use super::groups::CreatedId;
use super::policy_guard::{Guard, POLICY, Protection, protection_error};
use super::wire::Screen;
use super::writes::{
    WriteError, error_reason, guard_failed, guard_refusal, settle, unsettled, withheld_operand,
    write_operation,
};
use super::{Api, Client, Policy, Statement, invalid_input};
use crate::error::{RecoveryAdvice, SafeText};
use crate::wire::product::{
    evidence::Evidence,
    options::{Options, PathIds},
    redirect::Retry,
};
use crate::{ApiError, Error, Failure, RequestContext, WriteDispatch};

const POLICIES: &str = "policies-api/v1/policies";
const CREATE: &str = "iam.CreatePolicy";
const UPDATE: &str = "iam.UpdatePolicy";
const DELETE: &str = "iam.DeletePolicy";
const CREATE_ADVICE: &str = "iam.CreatePolicy: create may have already reached the server; check with list-policies --name before creating it again";
const UPDATE_ADVICE: &str = "iam.UpdatePolicy: vngcloud: write outcome not confirmed: replacement outcome is uncertain; read the policy before another write";

/// Defines a new customer IAM policy.
#[derive(Clone, Debug)]
pub struct CreatePolicyInput {
    /// Name of the new policy.
    pub name: String,
    /// Description of the policy.
    pub description: String,
    /// Required policy statements, in evaluation order.
    pub statements: Vec<Statement>,
}

impl CreatePolicyInput {
    /// Sets the required name and statements, with an empty description.
    pub fn new(name: impl Into<String>, statements: Vec<Statement>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            statements,
        }
    }
}

/// A newly created policy, with only its ID when confirmation failed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreatePolicyOutput {
    /// The policy returned by IAM.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

/// Replaces a customer IAM policy, preserving each omitted field.
#[derive(Clone, Debug)]
pub struct UpdatePolicyInput {
    /// ID of the policy to replace.
    pub policy_id: String,
    /// Replacement name, or absent to preserve the current name.
    pub name: Option<String>,
    /// Replacement description, or absent to preserve the current description.
    pub description: Option<String>,
    /// Replacement statements, or absent to preserve the current statements.
    pub statements: Option<Vec<Statement>>,
}

impl UpdatePolicyInput {
    /// Selects a policy and preserves all fields by default.
    pub fn new(policy_id: impl Into<String>) -> Self {
        Self {
            policy_id: policy_id.into(),
            name: None,
            description: None,
            statements: None,
        }
    }
}

/// The updated policy, with only its ID when confirmation failed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdatePolicyOutput {
    /// The policy returned by IAM.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

super::id_input!(
    /// Identifies an unattached customer policy to delete.
    DeletePolicyInput,
    /// ID of the policy to delete.
    policy_id
);

/// A completed policy deletion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeletePolicyOutput {}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Creates a non-privileged customer policy once, then confirms it by ID.
    pub async fn create_policy(
        &self,
        ctx: &RequestContext,
        input: CreatePolicyInput,
    ) -> Result<CreatePolicyOutput, Failure<CreatePolicyOutput>> {
        if input.name.is_empty() {
            return Err(guard_failed(invalid_input(format!(
                "{CREATE} requires Name"
            ))));
        }
        validate_statements(CREATE, &input.statements)?;
        let body = policy_body(&input.name, &input.description, &input.statements);
        let mut evidence = self.write_evidence(ctx, CREATE, &[], &[])?;
        if evidence.reflected_in(&body) {
            return Err(withheld_operand());
        }
        let mut guard = Guard::new(self, ctx, CREATE, &mut evidence).await?;
        guard.load_actions().await?;
        refuse_privileged(
            CREATE,
            guard.privileged(&input.statements)?,
            "the statements grant an IAM write action",
        )?;
        let mut operation = write_operation(
            CREATE,
            Method::POST,
            POLICIES,
            Retry::Once,
            &[StatusCode::CREATED],
        );
        operation.keep_status = true;
        let CreatedId(id) = self
            .write(
                ctx,
                Api::Policies,
                &operation,
                &[],
                Some(body),
                &mut evidence,
            )
            .await
            .map_err(|failure| uncertain_create(failure, &evidence))?;
        if !PathIds::WithUnderscore.allows(&id) || evidence.id_reflects(&id) {
            return Err(Failure::new(
                Error::from_api(ApiError::without_fallback(
                    CREATE,
                    None,
                    SafeText::new("", &[]),
                    SafeText::new(
                        "create response had no id; a policy may exist, check with list-policies --name",
                        &[],
                    ),
                    false,
                )),
                None,
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        let outcome = self.confirm_created_policy(ctx, &id, &mut evidence).await;
        outcome
            .map(|policy| CreatePolicyOutput { policy })
            .map_err(|failed| {
                settle(
                    failed,
                    partial_policy(&id, &evidence).map(|policy| CreatePolicyOutput { policy }),
                    &format!("{CREATE}: policy {id} was created but the read to confirm it failed"),
                    &evidence,
                )
            })
    }

    async fn confirm_created_policy<O>(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Policy, Failure<O>> {
        if !Options::DEFAULT.path_ids.allows(id) {
            return Err(guard_failed(invalid_input(
                "iam.GetPolicy requires a valid PolicyID".into(),
            )));
        }
        self.confirm_policy(ctx, id, evidence).await
    }

    /// Replaces a verified, non-managed policy once and confirms the result.
    pub async fn update_policy(
        &self,
        ctx: &RequestContext,
        input: UpdatePolicyInput,
    ) -> Result<UpdatePolicyOutput, Failure<UpdatePolicyOutput>> {
        let id = input.policy_id.as_str();
        if let Some(statements) = &input.statements {
            validate_statements(UPDATE, statements)?;
        }
        let mut evidence = self.write_evidence(
            ctx,
            UPDATE,
            &[("PolicyID", id)],
            &[
                input.name.as_deref().unwrap_or_default(),
                input.description.as_deref().unwrap_or_default(),
            ],
        )?;
        if input
            .statements
            .as_ref()
            .is_some_and(|statements| evidence.reflected_in(&wire_statements(statements)))
        {
            return Err(withheld_operand());
        }
        let mut guard = Guard::new(self, ctx, UPDATE, &mut evidence).await?;
        let current = guard.policy(id, true).await?;
        check_managed(UPDATE, &current)?;
        guard.load_actions().await?;
        let attachments = guard.attachments(id).await?;
        let protection = guard.attached_protection(attachments).await?;
        if protection == Protection::Caller {
            protection_error(UPDATE, protection, "")?;
        }
        refuse_privileged(
            UPDATE,
            guard.privileged(&current.statements)?,
            "the policy's current statements grant an IAM write action",
        )?;
        if let Some(statements) = &input.statements {
            refuse_privileged(
                UPDATE,
                guard.privileged(statements)?,
                "the proposed statements grant an IAM write action",
            )?;
        }
        protection_error(
            UPDATE,
            protection,
            "the policy is attached to a protected principal or group",
        )?;
        let body = policy_body(
            input.name.as_deref().unwrap_or(&current.name),
            input.description.as_deref().unwrap_or(&current.description),
            input.statements.as_deref().unwrap_or(&current.statements),
        );
        let mut op = write_operation(
            UPDATE,
            Method::PUT,
            POLICY,
            Retry::Once,
            &[StatusCode::NO_CONTENT],
        );
        op.keep_status = true;
        self.write::<Value, _>(ctx, Api::Policies, &op, &[id], Some(body), &mut evidence)
            .await
            .map_err(|failure| uncertain_update(failure, id, &evidence))?;
        self.confirm_policy(ctx, id, &mut evidence)
            .await
            .map(|policy| UpdatePolicyOutput { policy })
            .map_err(|failed| {
                settle(
                    failed,
                    partial_policy(id, &evidence).map(|policy| UpdatePolicyOutput { policy }),
                    &format!("{UPDATE}: policy {id} was updated but the read to confirm it failed"),
                    &evidence,
                )
            })
    }

    /// Deletes a customer policy after proving it has no attachments.
    pub async fn delete_policy(
        &self,
        ctx: &RequestContext,
        input: DeletePolicyInput,
    ) -> Result<DeletePolicyOutput, Failure<DeletePolicyOutput>> {
        let id = input.policy_id.as_str();
        let mut evidence = self.write_evidence(ctx, DELETE, &[("PolicyID", id)], &[])?;
        let mut guard = Guard::new(self, ctx, DELETE, &mut evidence).await?;
        check_managed(DELETE, &guard.policy(id, false).await?)?;
        if !guard.attachments(id).await?.is_empty() {
            return Err(guard_refusal(
                "iam: refused: resource in use: iam.DeletePolicy: the policy is attached",
                Some(WriteError::ResourceInUse),
            ));
        }
        let op = write_operation(
            DELETE,
            Method::DELETE,
            POLICY,
            Retry::Repeatable,
            &[StatusCode::NO_CONTENT],
        );
        self.write::<Value, _>(ctx, Api::Policies, &op, &[id], None, &mut evidence)
            .await?;
        Ok(DeletePolicyOutput {})
    }
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
fn validate_statements<O>(op: &str, statements: &[Statement]) -> Result<(), Failure<O>> {
    let invalid = |message| guard_failed(invalid_input(message));
    if statements.is_empty() {
        return Err(invalid(format!("{op} requires Statements")));
    }
    for (i, statement) in statements.iter().enumerate() {
        let reason = if !matches!(statement.effect.as_str(), "allow" | "deny") {
            "effect must be \"allow\" or \"deny\""
        } else if statement.actions.is_empty() {
            "has no actions"
        } else if statement.resources.is_empty() {
            "has no resources"
        } else if statement.actions.iter().any(String::is_empty) {
            "has an empty action"
        } else if statement.resources.iter().any(String::is_empty) {
            "has an empty resource"
        } else if statement.condition.as_ref().is_some_and(|operators| {
            operators
                .values()
                .any(|value| !value.is_object() && !value.is_null())
        }) {
            "has a condition operator that is neither an object nor null"
        } else {
            continue;
        };
        return Err(invalid(format!("{op}: statement {i} {reason}")));
    }
    Ok(())
}

fn policy_body(name: &str, description: &str, statements: &[Statement]) -> Value {
    json!({"name": name, "description": description, "statements": wire_statements(statements)})
}

fn wire_statements(statements: &[Statement]) -> Value {
    Value::Array(
        statements
            .iter()
            .map(|statement| {
                let mut fields = serde_json::Map::from_iter([
                    ("effect".into(), Value::from(statement.effect.as_str())),
                    ("actions".into(), Value::from(statement.actions.clone())),
                    ("resources".into(), Value::from(statement.resources.clone())),
                ]);
                if let Some(condition) = &statement.condition
                    && !condition.is_empty()
                {
                    fields.insert(
                        "condition".into(),
                        Value::Object(condition.clone().into_iter().collect()),
                    );
                }
                Value::Object(fields)
            })
            .collect(),
    )
}

fn partial_policy(id: &str, evidence: &Evidence) -> Option<Policy> {
    if evidence.id_reflects(id) {
        return None;
    }
    let mut policy = Policy::default();
    policy.id = id.to_owned();
    policy.screen(evidence);
    Some(policy)
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
fn check_managed<O>(op: &str, policy: &Policy) -> Result<(), Failure<O>> {
    if policy.managed() {
        return Err(guard_refusal(
            &format!("iam: refused: policy is managed: {op}: the policy is managed"),
            Some(WriteError::ManagedPolicy),
        ));
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
fn refuse_privileged<O>(op: &str, privileged: bool, reason: &str) -> Result<(), Failure<O>> {
    if privileged {
        protection_error(op, Protection::Privileged, reason)?;
    }
    Ok(())
}

fn uncertain_create<O>(failure: Failure<O>, evidence: &Evidence) -> Failure<O> {
    let (error, _, dispatch) = failure.into_parts();
    if dispatch == WriteDispatch::NotSent
        || observed_status(&error).is_some_and(|status| (400..500).contains(&status))
    {
        return Failure::new(error, None, dispatch);
    }
    let advice = format!("{CREATE_ADVICE}: {}", error_reason(&error));
    let advice = if evidence.can_retain_partial() && !evidence.text_reflects(&advice) {
        &advice
    } else {
        CREATE_ADVICE
    };
    Failure::new(
        error.with_outer_detail(RecoveryAdvice::new(advice)),
        None,
        dispatch,
    )
}

fn uncertain_update(
    failure: Failure<UpdatePolicyOutput>,
    id: &str,
    evidence: &Evidence,
) -> Failure<UpdatePolicyOutput> {
    let (error, _, dispatch) = failure.into_parts();
    if dispatch == WriteDispatch::NotSent
        || observed_status(&error)
            .is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 499))
    {
        return Failure::new(error, None, dispatch);
    }
    unsettled(
        error,
        partial_policy(id, evidence).map(|policy| UpdatePolicyOutput { policy }),
        evidence,
        |cause| format!("{UPDATE_ADVICE}: {}", error_reason(cause)),
    )
}

fn observed_status(error: &Error) -> Option<u16> {
    error
        .detail::<StatusCode>()
        .map(StatusCode::as_u16)
        .or_else(|| error.api_error().and_then(ApiError::status))
}
