use super::envelope::BudgetData;
use super::model::{field, null_default, reflects_integer};
use super::{Client, read_operation};
use crate::output_number;
use crate::wire::product::sender::SentCredentials;
use crate::wire::strict_json::Shape;
use crate::{Failure, List, RequestContext};
use serde::{Deserialize, Serialize, Serializer};

/// Selects the account's current billing period without filters.
#[derive(Clone, Debug, Default)]
pub struct GetCurrentPeriodCostInput {}

/// The account's actual and forecasted cost for its current period.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetCurrentPeriodCostOutput {
    /// Costs and dates for the current period.
    #[serde(rename = "PeriodCost")]
    pub period_cost: PeriodCost,
}

/// Dates and spend for the current billing period.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct PeriodCost {
    /// Billing period key in `YYYY-MM` form.
    #[serde(default, deserialize_with = "null_default")]
    pub period_key: String,
    /// Start of the billing period as reported by the API.
    #[serde(default, deserialize_with = "null_default")]
    pub period_start: String,
    /// End of the billing period as reported by the API.
    #[serde(default, deserialize_with = "null_default")]
    pub period_end: String,
    /// Actual spend in the account's billing currency.
    pub actual_cost: f64,
    /// Forecasted spend in the account's billing currency.
    pub forecasted_cost: f64,
}

/// Selects the budget whose thresholds to read.
#[derive(Clone, Debug)]
pub struct ListBudgetThresholdsInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
}

impl ListBudgetThresholdsInput {
    /// Stores the UUID for validation by [`Client::list_budget_thresholds`].
    pub fn new(budget_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
        }
    }
}

/// Configured budget thresholds without pagination metadata.
pub type ListBudgetThresholdsOutput = List<Threshold>;

/// The settings and notification state of a budget threshold.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct Threshold {
    /// Threshold UUID used in requests.
    #[serde(rename(serialize = "UUID"), deserialize_with = "null_default")]
    pub uuid: String,
    /// Spend measured by the threshold, such as `ACTUAL` or `FORECASTED`.
    #[serde(deserialize_with = "null_default")]
    pub threshold_type: String,
    /// Percentage of the budget at which the threshold triggers.
    #[serde(deserialize_with = "null_default")]
    pub threshold_percentage: f64,
    /// Comparison applied to the spend percentage, such as `GTE`.
    #[serde(deserialize_with = "null_default")]
    pub comparison_operator: String,
    /// Whether the threshold can send alerts.
    #[serde(deserialize_with = "null_default")]
    pub enabled: bool,
    /// Maximum number of alerts in a billing period.
    #[serde(deserialize_with = "null_default")]
    pub max_alerts_per_period: i64,
    /// Interval between alert reminders in hours.
    #[serde(deserialize_with = "null_default")]
    pub reminder_interval_hours: i64,
    /// Notification state reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub notification_state: String,
    /// Timestamp of the most recent alert, or empty when none is reported.
    #[serde(deserialize_with = "null_default")]
    pub last_alert_at: String,
}

/// Selects a budget and optionally filters its alerts by period.
#[derive(Clone, Debug)]
pub struct ListBudgetAlertsInput {
    /// Budget UUID, using only ASCII letters, digits, and hyphens.
    pub budget_uuid: String,
    /// Billing period in `YYYY-MM` form. An empty string includes all periods.
    pub period_key: String,
}

impl ListBudgetAlertsInput {
    /// Selects all periods for the budget. The request validates the UUID.
    pub fn new(budget_uuid: impl Into<String>) -> Self {
        Self {
            budget_uuid: budget_uuid.into(),
            period_key: String::new(),
        }
    }
}

/// Alerts for a budget, without pagination metadata.
pub type ListBudgetAlertsOutput = List<Alert>;

/// A notification sent when spend breached a budget threshold.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(from = "AlertWire", rename_all = "PascalCase")]
pub struct Alert {
    /// Time the threshold triggered, as reported by the API.
    pub triggered_at: String,
    /// Spend measured by the threshold, such as `ACTUAL` or `FORECASTED`.
    pub threshold_type: String,
    /// Percentage of the budget configured to trigger the alert.
    pub threshold_percentage: f64,
    /// Spend value that triggered the alert, in the billing currency.
    pub triggered_value: f64,
    /// Actual spend as a percentage of the budget.
    pub actual_percentage: f64,
    /// Delivery state reported by the API.
    pub delivery_status: String,
    /// Decoded recipient list. Absent when the API does not supply an array.
    #[serde(serialize_with = "serialize_recipients")]
    pub recipients: Option<Vec<String>>,
    /// Original recipients string when the API did not encode a valid list.
    pub recipients_raw: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AlertWire {
    #[serde(deserialize_with = "null_default")]
    triggered_at: String,
    #[serde(deserialize_with = "null_default")]
    threshold_type: String,
    #[serde(deserialize_with = "null_default")]
    threshold_percentage: f64,
    #[serde(deserialize_with = "null_default")]
    triggered_value: f64,
    #[serde(deserialize_with = "null_default")]
    actual_percentage: f64,
    #[serde(deserialize_with = "null_default")]
    delivery_status: String,
    #[serde(deserialize_with = "null_default")]
    recipients: String,
}

impl From<AlertWire> for Alert {
    fn from(wire: AlertWire) -> Self {
        let (recipients, recipients_raw) =
            match serde_json::from_str::<Option<Vec<Option<String>>>>(&wire.recipients) {
                Ok(items) => (
                    items.map(|items| items.into_iter().map(Option::unwrap_or_default).collect()),
                    String::new(),
                ),
                Err(_) => (None, wire.recipients),
            };
        Self {
            triggered_at: wire.triggered_at,
            threshold_type: wire.threshold_type,
            threshold_percentage: wire.threshold_percentage,
            triggered_value: wire.triggered_value,
            actual_percentage: wire.actual_percentage,
            delivery_status: wire.delivery_status,
            recipients,
            recipients_raw,
        }
    }
}

fn serialize_recipients<S: Serializer>(
    recipients: &Option<Vec<String>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    recipients
        .as_deref()
        .unwrap_or_default()
        .serialize(serializer)
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads the account's costs for the current billing period.
    pub async fn get_current_period_cost(
        &self,
        ctx: &RequestContext,
        _input: GetCurrentPeriodCostInput,
    ) -> Result<GetCurrentPeriodCostOutput, Failure<GetCurrentPeriodCostOutput>> {
        let period_cost = self
            .read(
                ctx,
                &read_operation(
                    "billing.GetCurrentPeriodCost",
                    "gateway/api/v1/budgets/cost/overview",
                ),
                &[],
                &[],
            )
            .await?;
        Ok(GetCurrentPeriodCostOutput { period_cost })
    }

    /// Reads a budget's thresholds. An invalid UUID fails before authentication.
    pub async fn list_budget_thresholds(
        &self,
        ctx: &RequestContext,
        input: ListBudgetThresholdsInput,
    ) -> Result<ListBudgetThresholdsOutput, Failure<ListBudgetThresholdsOutput>> {
        let items = self
            .read(
                ctx,
                &read_operation(
                    "billing.ListBudgetThresholds",
                    "gateway/api/v1/budgets/{}/thresholds",
                ),
                &[&input.budget_uuid],
                &[],
            )
            .await?;
        Ok(List { items })
    }

    /// Reads a budget's alerts for the selected period or all periods.
    pub async fn list_budget_alerts(
        &self,
        ctx: &RequestContext,
        input: ListBudgetAlertsInput,
    ) -> Result<ListBudgetAlertsOutput, Failure<ListBudgetAlertsOutput>> {
        let query = [("periodKey", input.period_key.as_str())];
        let query: &[_] = if input.period_key.is_empty() {
            &[]
        } else {
            &query
        };
        let items = self
            .read(
                ctx,
                &read_operation(
                    "billing.ListBudgetAlerts",
                    "gateway/api/v1/budgets/{}/alerts",
                ),
                &[&input.budget_uuid],
                query,
            )
            .await?;
        Ok(List { items })
    }
}

impl BudgetData for PeriodCost {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("periodKey"),
        field("periodStart"),
        field("periodEnd"),
        field("actualCost"),
        field("forecastedCost"),
    ]);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        ![self.actual_cost, self.forecasted_cost]
            .into_iter()
            .any(|value| output_number::reflects(value, credentials, true))
    }
}

impl Threshold {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("uuid"),
        field("thresholdType"),
        field("thresholdPercentage"),
        field("comparisonOperator"),
        field("enabled"),
        field("maxAlertsPerPeriod"),
        field("reminderIntervalHours"),
        field("notificationState"),
        field("lastAlertAt"),
    ]);

    fn reflects_numbers(&self, credentials: &SentCredentials) -> bool {
        output_number::reflects(self.threshold_percentage, credentials, true)
            || reflects_integer(self.max_alerts_per_period, credentials)
            || reflects_integer(self.reminder_interval_hours, credentials)
    }
}

impl BudgetData for Threshold {
    const SCHEMA: Shape<'static> = Threshold::SCHEMA;

    fn valid(&self, credentials: &SentCredentials) -> bool {
        !self.uuid.trim().is_empty()
            && !self.reflects_numbers(credentials)
            && !credentials.reflects(if self.enabled { b"true" } else { b"false" })
    }

    fn redact(&mut self, credentials: &SentCredentials) {
        for field in [
            &mut self.uuid,
            &mut self.threshold_type,
            &mut self.comparison_operator,
            &mut self.notification_state,
            &mut self.last_alert_at,
        ] {
            if credentials.reflects(field.as_bytes()) {
                *field = "<redacted>".into();
            }
        }
    }
}

impl BudgetData for Vec<Threshold> {
    const SCHEMA: Shape<'static> = Shape::Array(&Threshold::SCHEMA);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        self.iter().all(|row| !row.reflects_numbers(credentials))
    }
}

impl BudgetData for Vec<Alert> {
    const SCHEMA: Shape<'static> = Shape::Array(&Shape::Object(&[
        field("triggeredAt"),
        field("thresholdType"),
        field("thresholdPercentage"),
        field("triggeredValue"),
        field("actualPercentage"),
        field("deliveryStatus"),
        field("recipients"),
    ]));

    fn valid(&self, credentials: &SentCredentials) -> bool {
        self.iter().all(|row| {
            ![
                row.threshold_percentage,
                row.triggered_value,
                row.actual_percentage,
            ]
            .into_iter()
            .any(|value| output_number::reflects(value, credentials, true))
                && !row
                    .recipients
                    .iter()
                    .flatten()
                    .any(|recipient| credentials.reflects(recipient.as_bytes()))
        })
    }
}
