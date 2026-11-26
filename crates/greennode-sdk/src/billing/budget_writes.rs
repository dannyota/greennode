use super::Client;
use super::envelope::{BudgetData, Envelope};
use super::model::Budget;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation},
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Creates an account-wide budget with a limit in billing credits.
#[derive(Clone, Debug)]
pub struct CreateBudgetInput {
    /// Budget name, validated by the API.
    pub name: String,
    /// Billing period, such as `MONTHLY`.
    pub period_type: String,
    /// Budget cost type, such as `ACTUAL`.
    pub r#type: String,
    /// Nonzero limit in billing credits.
    pub limit_amount: i64,
    /// An empty status sends `ACTIVE`.
    pub status: String,
}

impl CreateBudgetInput {
    /// Sets the required fields and uses the default active status.
    pub fn new(
        name: impl Into<String>,
        period_type: impl Into<String>,
        budget_type: impl Into<String>,
        limit_amount: i64,
    ) -> Self {
        Self {
            name: name.into(),
            period_type: period_type.into(),
            r#type: budget_type.into(),
            limit_amount,
            status: String::new(),
        }
    }
}

/// Details returned for a confirmed create.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateBudgetOutput {
    /// Created budget, including the UUID returned by the API.
    #[serde(rename = "Budget")]
    pub budget: Budget,
}

/// Changes selected fields on an existing account-wide budget.
#[derive(Clone, Debug)]
pub struct UpdateBudgetInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
    /// Replacement name when set.
    pub name: Option<String>,
    /// Replacement billing period when set.
    pub period_type: Option<String>,
    /// Replacement cost type when set.
    pub r#type: Option<String>,
    /// Replacement limit in billing credits when set.
    pub limit_amount: Option<i64>,
    /// Replacement status, such as `PAUSED`, when set.
    pub status: Option<String>,
}

impl UpdateBudgetInput {
    /// Selects a budget. Set at least one change before calling the API.
    pub fn new(budget_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
            name: None,
            period_type: None,
            r#type: None,
            limit_amount: None,
            status: None,
        }
    }
}

/// A confirmed update with no returned budget fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateBudgetOutput {}

/// Selects a budget and its API-managed thresholds for deletion.
#[derive(Clone, Debug)]
pub struct DeleteBudgetInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
}

impl DeleteBudgetInput {
    /// Stores the UUID for validation by [`Client::delete_budget`].
    pub fn new(budget_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
        }
    }
}

/// A confirmed deletion with no returned budget fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteBudgetOutput {}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a budget once. Read current state before retrying an unconfirmed create.
    pub async fn create_budget(
        &self,
        ctx: &RequestContext,
        input: CreateBudgetInput,
    ) -> Result<CreateBudgetOutput, Failure<CreateBudgetOutput>> {
        if input.name.is_empty()
            || input.period_type.is_empty()
            || input.r#type.is_empty()
            || input.limit_amount == 0
        {
            return Err(not_sent(Error::new(ErrorClass::InvalidInput)));
        }
        let status = if input.status.is_empty() {
            "ACTIVE"
        } else {
            &input.status
        };
        let body = json!({
            "name": input.name,
            "periodType": input.period_type,
            "type": input.r#type,
            "limitAmount": input.limit_amount,
            "status": status,
        });
        let budget = self
            .write(
                ctx,
                &write_operation(
                    "billing.CreateBudget",
                    Method::POST,
                    "gateway/api/v1/budgets",
                    false,
                ),
                &[],
                Some(body),
            )
            .await?;
        Ok(CreateBudgetOutput { budget })
    }

    /// Sends only selected fields, once. A request with no changes fails locally.
    pub async fn update_budget(
        &self,
        ctx: &RequestContext,
        input: UpdateBudgetInput,
    ) -> Result<UpdateBudgetOutput, Failure<UpdateBudgetOutput>> {
        let body: Map<_, _> = [
            ("name", input.name.map(Value::from)),
            ("periodType", input.period_type.map(Value::from)),
            ("type", input.r#type.map(Value::from)),
            ("limitAmount", input.limit_amount.map(Value::from)),
            ("status", input.status.map(Value::from)),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value)))
        .collect();
        if body.is_empty() {
            return Err(not_sent(Error::new(ErrorClass::InvalidInput)));
        }
        self.write::<(), _>(
            ctx,
            &write_operation(
                "billing.UpdateBudget",
                Method::PUT,
                "gateway/api/v1/budgets/{}",
                true,
            ),
            &[&input.budget_uuid],
            Some(Value::Object(body)),
        )
        .await?;
        Ok(UpdateBudgetOutput {})
    }

    /// Deletes the budget and its API-managed thresholds in one request.
    pub async fn delete_budget(
        &self,
        ctx: &RequestContext,
        input: DeleteBudgetInput,
    ) -> Result<DeleteBudgetOutput, Failure<DeleteBudgetOutput>> {
        self.write::<(), _>(
            ctx,
            &write_operation(
                "billing.DeleteBudget",
                Method::DELETE,
                "gateway/api/v1/budgets/{}",
                true,
            ),
            &[&input.budget_uuid],
            None,
        )
        .await?;
        Ok(DeleteBudgetOutput {})
    }

    async fn write<T: BudgetData, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        body: Option<Value>,
    ) -> Result<T, Failure<O>> {
        let session = &self.session;
        let base = session
            .endpoints()
            .billing
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let body = match body {
            Some(body) => serde_json::to_vec(&body)
                .map(Bytes::from)
                .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?,
            None => Bytes::new(),
        };
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        operation::execute::<Envelope<T>>(
            runtime,
            base,
            operation,
            path,
            &[],
            session.bearer_source(),
            body,
        )
        .await
        .map(|(_, envelope)| envelope.data)
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            let error = if dispatch == WriteDispatch::MayHaveBeenSent {
                reconcile(operation, error)
            } else {
                error
            };
            Failure::new(error, None, dispatch)
        })
    }
}

fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

fn reconcile(operation: &Operation, error: Error) -> Error {
    let api = error.api_error();
    let status = error
        .detail::<StatusCode>()
        .copied()
        .or_else(|| api.and_then(|api| StatusCode::from_u16(api.status()?).ok()));
    if status.is_some_and(|status| status.is_client_error()) {
        return error;
    }
    let code = if status.is_some_and(|status| {
        status.is_redirection() || (status.is_success() && error.is_body_too_large())
    }) {
        "InvalidResponse"
    } else {
        api.map_or("", ApiError::code)
    };
    Error::from_api(ApiError::new(
        operation.id,
        status.map(|status| status.as_u16()),
        SafeText::new(code, &[]),
        SafeText::new(
            "budget write outcome is unconfirmed; read current state before retrying",
            &[],
        ),
        false,
    ))
    .with_cause(error)
}

fn write_operation(
    id: &'static str,
    method: Method,
    template: &'static str,
    acknowledgement: bool,
) -> Operation {
    Operation {
        id,
        template,
        method,
        class: Classification::Write,
        retry: Retry::Once,
        success: &[StatusCode::OK, StatusCode::CREATED, StatusCode::NO_CONTENT],
        cap: 8 << 20,
        allow_recovery: false,
        allow_empty: acknowledgement,
        no_output: acknowledgement,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: false,
        reject_reflected_credentials: true,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}
