use super::budget_writes::{not_sent, write_operation};
use super::{Client, Threshold};
use crate::wire::product::operation::Operation;
use crate::{Error, ErrorClass, Failure, RequestContext};
use http::Method;
use serde::Serialize;
use serde_json::{Map, Value, json};

const THRESHOLDS: &str = "gateway/api/v1/budgets/{}/thresholds";
const THRESHOLD: &str = "gateway/api/v1/budgets/{}/thresholds/{}";

/// Creates an alert threshold on a budget.
#[derive(Clone)]
pub struct CreateBudgetThresholdInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
    /// Spend measured by the threshold, such as `ACTUAL` or `FORECASTED`.
    pub threshold_type: String,
    /// Nonzero alert percentage. The API enforces its allowed range.
    pub threshold_percentage: i64,
    /// Maximum alerts per period. Zero sends 1.
    pub max_alerts_per_period: i64,
    /// Reminder interval in hours. Zero sends 648.
    pub reminder_interval_hours: i64,
}

crate::error::debug_record!(CreateBudgetThresholdInput {
    budget_uuid,
    threshold_type,
    threshold_percentage,
    max_alerts_per_period,
    reminder_interval_hours
});

impl CreateBudgetThresholdInput {
    /// Sets the required fields and uses the console's alert and reminder defaults.
    pub fn new(
        budget_uuid: impl Into<String>,
        threshold_type: impl Into<String>,
        threshold_percentage: i64,
    ) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
            threshold_type: threshold_type.into(),
            threshold_percentage,
            max_alerts_per_period: 0,
            reminder_interval_hours: 0,
        }
    }
}

/// Details returned for a confirmed threshold create.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateBudgetThresholdOutput {
    /// The created threshold, including its assigned UUID.
    #[serde(rename = "Threshold")]
    pub threshold: Threshold,
}

crate::wire::product::output::public_fields!(CreateBudgetThresholdOutput { threshold });

/// Changes selected fields on an existing budget threshold.
#[derive(Clone)]
pub struct UpdateBudgetThresholdInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
    /// Threshold UUID, using only ASCII letters, digits, and hyphens.
    pub threshold_uuid: String,
    /// Replacement alert percentage when set.
    pub threshold_percentage: Option<i64>,
    /// Whether the threshold can send alerts, when set.
    pub enabled: Option<bool>,
    /// Replacement maximum alerts per period when set.
    pub max_alerts_per_period: Option<i64>,
    /// Replacement reminder interval in hours when set.
    pub reminder_interval_hours: Option<i64>,
}

crate::error::debug_record!(UpdateBudgetThresholdInput {
    budget_uuid,
    threshold_uuid,
    threshold_percentage,
    enabled,
    max_alerts_per_period,
    reminder_interval_hours
});

impl UpdateBudgetThresholdInput {
    /// Selects a threshold. Set at least one change before calling the API.
    pub fn new(budget_uuid: impl Into<String>, threshold_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
            threshold_uuid: threshold_uuid.into(),
            threshold_percentage: None,
            enabled: None,
            max_alerts_per_period: None,
            reminder_interval_hours: None,
        }
    }
}

/// A confirmed update with no returned fields.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct UpdateBudgetThresholdOutput {}

crate::wire::product::output::public_fields!(UpdateBudgetThresholdOutput {});

/// Selects a threshold to delete.
#[derive(Clone)]
pub struct DeleteBudgetThresholdInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
    /// Threshold UUID, using only ASCII letters, digits, and hyphens.
    pub threshold_uuid: String,
}

crate::error::debug_record!(DeleteBudgetThresholdInput {
    budget_uuid,
    threshold_uuid
});

impl DeleteBudgetThresholdInput {
    /// Stores both UUIDs for validation by [`Client::delete_budget_threshold`].
    pub fn new(budget_uuid: impl Into<String>, threshold_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
            threshold_uuid: threshold_uuid.into(),
        }
    }
}

/// A confirmed deletion with no returned fields.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DeleteBudgetThresholdOutput {}

crate::wire::product::output::public_fields!(DeleteBudgetThresholdOutput {});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a threshold with GTE comparison. List thresholds before retrying an unconfirmed write.
    pub async fn create_budget_threshold(
        &self,
        ctx: &RequestContext,
        input: CreateBudgetThresholdInput,
    ) -> Result<CreateBudgetThresholdOutput, Failure<CreateBudgetThresholdOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.threshold_type.is_empty() || input.threshold_percentage == 0 {
                return Err(not_sent(Error::new(ErrorClass::InvalidInput)));
            }
            let max_alerts = match input.max_alerts_per_period {
                0 => 1,
                value => value,
            };
            let reminder = match input.reminder_interval_hours {
                0 => 648,
                value => value,
            };
            let body = json!({
                "thresholdType": input.threshold_type,
                "thresholdPercentage": input.threshold_percentage,
                "comparisonOperator": "GTE",
                "maxAlertsPerPeriod": max_alerts,
                "reminderIntervalHours": reminder,
            });
            let threshold = self
                .write(
                    ctx,
                    &Operation {
                        reflection: crate::wire::product::operation::Reflection::Redact,
                        ..write_operation(
                            "billing.CreateBudgetThreshold",
                            Method::POST,
                            THRESHOLDS,
                            false,
                        )
                    },
                    &[&input.budget_uuid],
                    Some(body),
                )
                .await?;
            Ok(CreateBudgetThresholdOutput { threshold })
        })
    }

    /// Sends only selected fields, once. A request with no changes fails locally.
    pub async fn update_budget_threshold(
        &self,
        ctx: &RequestContext,
        input: UpdateBudgetThresholdInput,
    ) -> Result<UpdateBudgetThresholdOutput, Failure<UpdateBudgetThresholdOutput>> {
        crate::wire::product::exit::finish!(async {
            let body: Map<_, _> = [
                (
                    "thresholdPercentage",
                    input.threshold_percentage.map(Value::from),
                ),
                ("enabled", input.enabled.map(Value::from)),
                (
                    "maxAlertsPerPeriod",
                    input.max_alerts_per_period.map(Value::from),
                ),
                (
                    "reminderIntervalHours",
                    input.reminder_interval_hours.map(Value::from),
                ),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value)))
            .collect();
            if body.is_empty() {
                return Err(not_sent(Error::new(ErrorClass::InvalidInput).with_detail(
                    "greennode: invalid input: billing.UpdateBudgetThreshold requires at least one field to change",
                )));
            }
            self.write::<(), _>(
                ctx,
                &write_operation(
                    "billing.UpdateBudgetThreshold",
                    Method::PUT,
                    THRESHOLD,
                    true,
                ),
                &[&input.budget_uuid, &input.threshold_uuid],
                Some(Value::Object(body)),
            )
            .await?;
            Ok(UpdateBudgetThresholdOutput {})
        })
    }

    /// Deletes the threshold in one request.
    pub async fn delete_budget_threshold(
        &self,
        ctx: &RequestContext,
        input: DeleteBudgetThresholdInput,
    ) -> Result<DeleteBudgetThresholdOutput, Failure<DeleteBudgetThresholdOutput>> {
        crate::wire::product::exit::finish!(async {
            self.write::<(), _>(
                ctx,
                &write_operation(
                    "billing.DeleteBudgetThreshold",
                    Method::DELETE,
                    THRESHOLD,
                    true,
                ),
                &[&input.budget_uuid, &input.threshold_uuid],
                None,
            )
            .await?;
            Ok(DeleteBudgetThresholdOutput {})
        })
    }
}
