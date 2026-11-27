use crate::wire::product::sender::SentCredentials;
use crate::wire::strict_json::{Shape, ShapeField};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

/// An account's configured spend cap and its available period summary.
#[derive(Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct Budget {
    /// Budget UUID used in requests.
    #[serde(rename(serialize = "UUID"), deserialize_with = "null_default")]
    pub uuid: String,
    /// Numeric budget ID.
    #[serde(rename(serialize = "ID"), deserialize_with = "null_default")]
    pub id: i64,
    /// Display name configured on the budget.
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Budget period, such as `MONTHLY` or `QUARTERLY`.
    #[serde(deserialize_with = "null_default")]
    pub period_type: String,
    /// Budget type, such as `ACTUAL` or `FORECASTED`.
    #[serde(
        rename(deserialize = "type", serialize = "Type"),
        deserialize_with = "null_default"
    )]
    pub r#type: String,
    /// Exact spend cap in whole currency units.
    #[serde(deserialize_with = "limit_amount")]
    pub limit_amount: i64,
    /// Currency reported by billing.
    #[serde(deserialize_with = "null_default")]
    pub currency: String,
    /// Current state, such as `ACTIVE` or `PAUSED`.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    /// Billing period key in `YYYY-MM` form when reported.
    #[serde(deserialize_with = "null_default")]
    pub period_key: String,
    /// Start of the billing period as reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub period_start: String,
    /// End of the billing period as reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub period_end: String,
    /// Budget start date as reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub start_date: String,
    /// Creation timestamp as reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub created_at: String,
    /// Last update timestamp as reported by the API.
    #[serde(deserialize_with = "null_default")]
    pub updated_at: String,
    /// Actual spend, when reported.
    pub actual_cost: Option<f64>,
    /// Forecasted spend, when reported.
    pub forecasted_cost: Option<f64>,
    /// Actual spend as a percentage of the cap, when reported.
    pub actual_percentage: Option<f64>,
    /// Forecasted spend as a percentage of the cap, when reported.
    pub forecasted_percentage: Option<f64>,
    /// Threshold percentage, when reported.
    pub threshold_percentage: Option<f64>,
    /// Whether the budget has an alarm.
    #[serde(deserialize_with = "null_default")]
    pub alarm: bool,
    /// Number of configured thresholds.
    #[serde(deserialize_with = "null_default")]
    pub threshold_count: i64,
    /// Number of alarm thresholds.
    #[serde(deserialize_with = "null_default")]
    pub alarm_threshold_count: i64,
}

crate::wire::product::output::public_fields!(Budget {
    uuid,
    id,
    name,
    period_type,
    r#type,
    limit_amount,
    currency,
    status,
    period_key,
    period_start,
    period_end,
    start_date,
    created_at,
    updated_at,
    actual_cost,
    forecasted_cost,
    actual_percentage,
    forecasted_percentage,
    threshold_percentage,
    alarm,
    threshold_count,
    alarm_threshold_count
});

impl Budget {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("uuid"),
        field("id"),
        field("name"),
        field("periodType"),
        field("type"),
        field("limitAmount"),
        field("currency"),
        field("status"),
        field("periodKey"),
        field("periodStart"),
        field("periodEnd"),
        field("startDate"),
        field("createdAt"),
        field("updatedAt"),
        field("actualCost"),
        field("forecastedCost"),
        field("actualPercentage"),
        field("forecastedPercentage"),
        field("thresholdPercentage"),
        field("alarm"),
        field("thresholdCount"),
        field("alarmThresholdCount"),
    ]);

    pub(super) fn reflects_numbers(&self, credentials: &SentCredentials) -> bool {
        if credentials.values().is_empty() {
            return false;
        }
        [
            self.id,
            self.limit_amount,
            self.threshold_count,
            self.alarm_threshold_count,
        ]
        .into_iter()
        .any(|value| reflects_integer(value, credentials))
            || [
                self.actual_cost,
                self.forecasted_cost,
                self.actual_percentage,
                self.forecasted_percentage,
                self.threshold_percentage,
            ]
            .into_iter()
            .flatten()
            .any(|value| crate::output_number::reflects(value, credentials, true))
    }
}

pub(super) const fn field(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}

pub(super) fn reflects_integer(value: i64, credentials: &SentCredentials) -> bool {
    credentials.integer_output_reflects(value)
}

pub(super) fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn limit_amount<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let raw = <&RawValue>::deserialize(deserializer)?;
    exact_integer(raw.get()).ok_or_else(|| serde::de::Error::custom("invalid limitAmount"))
}

pub(super) fn exact_integer(raw: &str) -> Option<i64> {
    if raw.len() > 128 || !matches!(raw.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
        return None;
    }
    let (mantissa, exponent) = match raw.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (raw, 0),
    };
    if !(-128..=128).contains(&exponent) {
        return None;
    }
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = whole.bytes().chain(fraction.bytes());
    let shift = exponent - i32::try_from(fraction.len()).ok()?;
    let keep = (whole.len() + fraction.len()).saturating_sub(shift.min(0).unsigned_abs() as usize);
    if digits.clone().skip(keep).any(|digit| digit != b'0') {
        return None;
    }
    let zeros = std::iter::repeat_n(b'0', usize::try_from(shift.max(0)).ok()?);
    let number = digits
        .take(keep)
        .chain(zeros)
        .try_fold(0_i64, |sum, digit| {
            let digit = digit.checked_sub(b'0')?;
            sum.checked_mul(10)?.checked_sub(i64::from(digit))
        })?;
    if negative {
        Some(number)
    } else {
        number.checked_neg()
    }
}
