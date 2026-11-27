//! Manage budgets and read spend summaries for the authenticated account.

mod budget_reads;
mod budget_writes;
mod cost;
mod envelope;
mod history;
mod model;
#[cfg(test)]
mod tests;
mod threshold_writes;

pub use budget_reads::{
    Alert, GetCurrentPeriodCostInput, GetCurrentPeriodCostOutput, ListBudgetAlertsInput,
    ListBudgetAlertsOutput, ListBudgetThresholdsInput, ListBudgetThresholdsOutput, PeriodCost,
    Threshold,
};
pub use budget_writes::{
    CreateBudgetInput, CreateBudgetOutput, DeleteBudgetInput, DeleteBudgetOutput,
    UpdateBudgetInput, UpdateBudgetOutput,
};
pub use cost::{
    Balances, CostBreakdown, CostResource, CostResourceSummary, CostSeries, CostSeriesGroup,
    CostSeriesPoint, CostSummary, GetBalancesInput, GetBalancesOutput, GetCostOverviewInput,
    GetCostOverviewOutput, ListCostResourcesInput, ListCostResourcesOutput,
};
pub use history::{
    DIRECTION_CREDIT, DIRECTION_DEBIT, GATEWAY_BALANCE, ListPaymentsInput, ListPaymentsOutput,
    ListResourcesInput, ListResourcesOutput, ListTransactionsInput, ListTransactionsOutput,
    Payment, RENEW_TYPE_AUTO_RENEW, RENEW_TYPE_MANUAL, RENEW_TYPE_NON_RENEWABLE, Resource,
    ResourceBillingElement, ResourceThresholds, TRANSACTION_CHARGE, TRANSACTION_PURCHASE,
    TRANSACTION_REFUND, Transaction,
};
pub use model::Budget;
pub use threshold_writes::{
    CreateBudgetThresholdInput, CreateBudgetThresholdOutput, DeleteBudgetThresholdInput,
    DeleteBudgetThresholdOutput, UpdateBudgetThresholdInput, UpdateBudgetThresholdOutput,
};

use crate::config::Session;
use crate::wire::product::{
    operation::{self, Operation},
    redirect::{Classification, Retry},
    retry,
};
use crate::{Config, Error, ErrorClass, Failure, List, RequestContext, WriteDispatch};
use envelope::{BudgetData, Envelope, invalid_response};
use http::{Method, StatusCode};
use serde::Serialize;
use std::sync::Arc;

/// Client for account-wide billing operations.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

/// Filters a budget summary list.
#[derive(Clone, Debug, Default)]
pub struct ListBudgetsInput {
    /// Budget status to send to the API. An empty value includes all statuses.
    pub status: String,
}

/// Budget summaries without pagination metadata.
pub type ListBudgetsOutput = List<Budget>;

/// Identifies a budget to read.
#[derive(Clone, Debug)]
pub struct GetBudgetInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
}

impl GetBudgetInput {
    /// Stores the UUID for validation by [`Client::get_budget`].
    pub fn new(budget_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
        }
    }
}

/// Details of a single budget.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetBudgetOutput {
    /// Budget details with costs and percentages when the API supplies them.
    #[serde(rename = "Budget")]
    pub budget: Budget,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Shares the configuration's authentication and connections without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }

    /// Reads account-wide summaries, with an empty list when no budgets match.
    pub async fn list_budgets(
        &self,
        ctx: &RequestContext,
        input: ListBudgetsInput,
    ) -> Result<ListBudgetsOutput, Failure<ListBudgetsOutput>> {
        let mut query = vec![("view", "summary")];
        if !input.status.is_empty() {
            query.push(("status", input.status.as_str()));
        }
        let items = self
            .read(
                ctx,
                &read_operation("billing.ListBudgets", "gateway/api/v1/budgets"),
                &[],
                &query,
            )
            .await?;
        Ok(List { items })
    }

    /// Reads an account-wide budget. An invalid path ID fails before authentication.
    pub async fn get_budget(
        &self,
        ctx: &RequestContext,
        input: GetBudgetInput,
    ) -> Result<GetBudgetOutput, Failure<GetBudgetOutput>> {
        let budget = self
            .read(
                ctx,
                &read_operation("billing.GetBudget", "gateway/api/v1/budgets/{}"),
                &[&input.budget_uuid],
                &[],
            )
            .await?;
        Ok(GetBudgetOutput { budget })
    }

    async fn read<T: BudgetData, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
        let session = &self.session;
        let base = session
            .endpoints()
            .billing
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute::<Envelope<T>>(
            runtime,
            base,
            operation,
            path,
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, envelope)| envelope.data)
        .map_err(|error| {
            let (error, _, dispatch) = error.into_parts();
            let error = match error.detail::<StatusCode>().copied() {
                Some(status)
                    if status.is_success()
                        && error.class() == ErrorClass::Api
                        && error
                            .api_error()
                            .is_none_or(|api| api.code() != "InvalidResponse") =>
                {
                    invalid_response(operation, status, error.is_body_too_large()).with_cause(error)
                }
                _ => error,
            };
            Failure::new(error, None, dispatch)
        })
    }
}

fn read_operation(id: &'static str, template: &'static str) -> Operation {
    Operation {
        id,
        template,
        method: Method::GET,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: &[StatusCode::OK],
        cap: 8 << 20,
        allow_recovery: true,
        allow_empty: false,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: false,
        reflection: crate::wire::product::operation::Reflection::Refuse,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}
