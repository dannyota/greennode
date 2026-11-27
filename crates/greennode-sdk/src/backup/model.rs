use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::value::RawValue;

use crate::wire::product::sender::SentCredentials;

/// A Backup Center storage backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct Backend {
    /// Backend identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "id"),
        default,
        deserialize_with = "default_if_null"
    )]
    pub id: String,
    /// Backend display name.
    #[serde(default, deserialize_with = "default_if_null")]
    pub name: String,
}

/// A Backup Center policy and its verified schedule settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct Policy {
    /// Policy identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "id"),
        default,
        deserialize_with = "default_if_null"
    )]
    pub id: String,
    /// Storage backend identifier.
    #[serde(
        rename(serialize = "BackendID", deserialize = "backendId"),
        default,
        deserialize_with = "default_if_null"
    )]
    pub backend_id: String,
    /// Project associated with the policy.
    #[serde(
        rename(serialize = "ProjectID", deserialize = "projectId"),
        default,
        deserialize_with = "default_if_null"
    )]
    pub project_id: String,
    /// Product covered by the policy.
    #[serde(default, deserialize_with = "default_if_null")]
    pub product: String,
    /// Policy display name.
    #[serde(default, deserialize_with = "default_if_null")]
    pub name: String,
    /// Whether this is a default policy.
    #[serde(default, deserialize_with = "default_if_null")]
    pub is_default: bool,
    /// Number of backup instances associated with the policy.
    #[serde(default, deserialize_with = "default_if_null")]
    pub backup_instance_count: i64,
    /// Verified schedule settings.
    #[serde(default, deserialize_with = "default_if_null")]
    pub config: PolicyConfig,
    /// Creation timestamp as supplied by the API.
    #[serde(default, deserialize_with = "default_if_null")]
    pub created_at: String,
    /// Last update timestamp as supplied by the API.
    #[serde(default, deserialize_with = "default_if_null")]
    pub updated_at: String,
}

/// Schedule start time and cadence flags. Only daily settings are projected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct PolicyConfig {
    /// Hour of the scheduled start.
    #[serde(default, deserialize_with = "whole")]
    pub hour: i64,
    /// Minute of the scheduled start.
    #[serde(default, deserialize_with = "whole")]
    pub minute: i64,
    /// Time zone of the scheduled start.
    #[serde(default, deserialize_with = "default_if_null")]
    pub time_zone: String,
    /// Whether hourly backups are enabled.
    #[serde(default, deserialize_with = "default_if_null")]
    pub hourly_enabled: bool,
    /// Whether daily backups are enabled.
    #[serde(default, deserialize_with = "default_if_null")]
    pub daily_enabled: bool,
    /// Whether weekly backups are enabled.
    #[serde(default, deserialize_with = "default_if_null")]
    pub weekly_enabled: bool,
    /// Whether monthly backups are enabled.
    #[serde(default, deserialize_with = "default_if_null")]
    pub monthly_enabled: bool,
    /// Whether server protection is enabled.
    #[serde(default, deserialize_with = "default_if_null")]
    pub is_protected_server: bool,
    /// Daily schedule, if supplied by the API.
    pub daily_config: Option<DailyConfig>,
}

/// Daily backup settings with absent values preserved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct DailyConfig {
    /// Retention setting, including an explicit zero.
    #[serde(default, deserialize_with = "optional_whole")]
    pub retention: Option<i64>,
    /// Backup type, including an explicit empty string.
    pub backup_type: Option<String>,
    /// Incremental backup quantity, including an explicit zero.
    #[serde(default, deserialize_with = "optional_whole")]
    pub incremental_quantity: Option<i64>,
}

impl Policy {
    pub(super) fn reflects(&self, credentials: &SentCredentials) -> bool {
        [&self.id, &self.backend_id, &self.project_id]
            .into_iter()
            .any(|id| credentials.id_reflects(id))
            || [
                self.product.as_str(),
                &self.name,
                &self.created_at,
                &self.updated_at,
                &self.config.time_zone,
            ]
            .into_iter()
            .chain(
                self.config
                    .daily_config
                    .as_ref()
                    .and_then(|daily| daily.backup_type.as_deref()),
            )
            .any(|text| credentials.text_reflects(text))
            || [
                self.is_default,
                self.config.hourly_enabled,
                self.config.daily_enabled,
                self.config.weekly_enabled,
                self.config.monthly_enabled,
                self.config.is_protected_server,
            ]
            .into_iter()
            .any(|value| credentials.text_reflects(if value { "true" } else { "false" }))
            || [
                self.backup_instance_count,
                self.config.hour,
                self.config.minute,
            ]
            .into_iter()
            .chain(self.config.daily_config.iter().flat_map(|daily| {
                [daily.retention, daily.incremental_quantity]
                    .into_iter()
                    .flatten()
            }))
            .any(|value| reflects_integer(value, credentials))
    }
}

pub(super) fn reflects_integer(value: i64, credentials: &SentCredentials) -> bool {
    credentials.integer_output_reflects(value)
}

fn default_if_null<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn whole<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let raw = <&RawValue>::deserialize(deserializer)?;
    parse_whole(raw.get()).ok_or_else(|| de::Error::custom("invalid schedule integer"))
}

fn optional_whole<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Option::<&RawValue>::deserialize(deserializer)?
        .map(|raw| {
            parse_whole(raw.get()).ok_or_else(|| de::Error::custom("invalid schedule integer"))
        })
        .transpose()
}

fn parse_whole(raw: &str) -> Option<i64> {
    if raw.len() > 128 {
        return None;
    }
    let (negative, magnitude) = raw
        .strip_prefix('-')
        .map_or((false, raw), |rest| (true, rest));
    let (mantissa, exponent) = match magnitude.split_once(['e', 'E']) {
        Some((mantissa, power)) => (mantissa, power.parse::<i32>().ok()?),
        None => (magnitude, 0),
    };
    if !(-64..=64).contains(&exponent) {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = [whole, fraction].concat();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if digits.bytes().all(|byte| byte == b'0') {
        return Some(0);
    }
    let shift = exponent - i32::try_from(fraction.len()).ok()?;
    if shift < 0 {
        let keep = digits.len().checked_sub(usize::try_from(-shift).ok()?)?;
        if !digits.get(keep..)?.bytes().all(|byte| byte == b'0') {
            return None;
        }
        digits.truncate(keep);
    } else {
        digits.extend(std::iter::repeat_n('0', usize::try_from(shift).ok()?));
    }
    if negative {
        digits.insert(0, '-');
    }
    digits.parse().ok()
}
