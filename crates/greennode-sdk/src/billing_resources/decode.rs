use crate::model::wire::JsonSource;
use crate::model::wire::{exact_integer, null_default};
use crate::wire::strict_json::{self, Shape, ShapeField};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

use super::RenewPeriod;

/// Prepaid resource renewal state and aggregate counts.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase", serialize = "PascalCase"))]
pub struct ListResourcesOutput {
    /// Resources in API order.
    #[serde(rename(deserialize = "data", serialize = "Items"))]
    pub items: Vec<Resource>,
    /// Resources with auto-renew enabled, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub total_auto_renews: Option<i64>,
    /// Resources in good standing, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub total_good_resources: Option<i64>,
    /// Resources in a warning state, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub total_warning_resources: Option<i64>,
    /// Resources in an alert state, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub total_alerted_resources: Option<i64>,
    /// Expired resources, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub total_expired_resources: Option<i64>,
    /// Threshold data as reported by billing.
    #[serde(default, deserialize_with = "null_default")]
    pub extra: ResourceThresholds,
}

/// A prepaid resource's identity, renewal state, and billing details.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(deserialize = "camelCase", serialize = "PascalCase")
)]
pub struct Resource {
    /// Resource identifier.
    #[serde(rename(serialize = "ArtifactID"), deserialize_with = "null_default")]
    pub artifact_id: String,
    /// Resource name.
    #[serde(deserialize_with = "null_default")]
    pub artifact_name: String,
    /// Resource type.
    #[serde(deserialize_with = "null_default")]
    pub artifact_type: String,
    /// Product identifier.
    #[serde(deserialize_with = "null_default")]
    pub product: String,
    /// Renewal mode reported by billing.
    #[serde(deserialize_with = "null_default")]
    pub renew_type: String,
    /// Billing mode reported by billing.
    #[serde(deserialize_with = "null_default")]
    pub billing_type: String,
    /// Billing start time in epoch milliseconds, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub start_billing_time: Option<i64>,
    /// Billing end time in epoch milliseconds, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub end_billing_time: Option<i64>,
    /// Days remaining, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub date_left: Option<i64>,
    /// Renewal period, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub renew_period: Option<i64>,
    /// Billing channel, when reported.
    #[serde(default, deserialize_with = "integer")]
    pub channel: Option<i64>,
    /// Whether renewal is in progress, when reported.
    pub is_renewing: Option<bool>,
    /// Reported cost field with unverified meaning and currency.
    pub cost: Option<f64>,
    /// Raw status value, when present.
    #[serde(deserialize_with = "raw_value", serialize_with = "serialize_raw")]
    pub status: Option<Box<RawValue>>,
    /// Raw status display value, when present.
    #[serde(
        rename(deserialize = "statusUI", serialize = "StatusUI"),
        deserialize_with = "raw_value",
        serialize_with = "serialize_raw"
    )]
    pub status_ui: Option<Box<RawValue>>,
    /// Billing elements in API order.
    #[serde(deserialize_with = "billing_elements")]
    pub billing_elements: Vec<ResourceBillingElement>,
}

/// One billing element of a prepaid resource.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(deserialize = "camelCase", serialize = "PascalCase")
)]
pub struct ResourceBillingElement {
    /// Stock keeping unit.
    #[serde(rename(serialize = "SKU"), deserialize_with = "null_default")]
    pub sku: String,
    /// Raw quantity value, when present.
    #[serde(deserialize_with = "raw_value", serialize_with = "serialize_raw")]
    pub quantity: Option<Box<RawValue>>,
    /// Raw metadata, when present.
    #[serde(deserialize_with = "raw_value", serialize_with = "serialize_raw")]
    pub meta_key: Option<Box<RawValue>>,
}

/// Warning and alarm thresholds supplied with prepaid resources.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(deserialize = "camelCase", serialize = "PascalCase")
)]
pub struct ResourceThresholds {
    /// Raw warning threshold data, when present.
    #[serde(deserialize_with = "raw_value", serialize_with = "serialize_raw")]
    pub warning_thresholds: Option<Box<RawValue>>,
    /// Raw alarm threshold data, when present.
    #[serde(deserialize_with = "raw_value", serialize_with = "serialize_raw")]
    pub alarm_thresholds: Option<Box<RawValue>>,
}

crate::wire::product::output::public_fields!(ListResourcesOutput {
    items,
    total_auto_renews,
    total_good_resources,
    total_warning_resources,
    total_alerted_resources,
    total_expired_resources,
    extra
});

crate::wire::product::output::public_fields!(Resource {
    artifact_id,
    artifact_name,
    artifact_type,
    product,
    renew_type,
    billing_type,
    start_billing_time,
    end_billing_time,
    date_left,
    renew_period,
    channel,
    is_renewing,
    cost,
    status,
    status_ui,
    billing_elements
});

crate::wire::product::output::public_fields!(ResourceBillingElement {
    sku,
    quantity,
    meta_key
});

crate::wire::product::output::public_fields!(ResourceThresholds {
    warning_thresholds,
    alarm_thresholds
});

const EXTRA: Shape<'static> =
    Shape::Object(&[field("warningThresholds"), field("alarmThresholds")]);

const BILLING_ELEMENT: Shape<'static> =
    Shape::Object(&[field("sku"), field("quantity"), field("metaKey")]);

const RESOURCE: Shape<'static> = Shape::Object(&[
    field("artifactId"),
    field("artifactName"),
    field("artifactType"),
    field("product"),
    field("renewType"),
    field("billingType"),
    field("startBillingTime"),
    field("endBillingTime"),
    field("dateLeft"),
    field("renewPeriod"),
    field("channel"),
    field("isRenewing"),
    field("cost"),
    field("status"),
    field("statusUI"),
    ShapeField {
        name: "billingElements",
        shape: Shape::Array(&BILLING_ELEMENT),
    },
]);

const DATA: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "data",
        shape: Shape::Array(&RESOURCE),
    },
    field("totalAutoRenews"),
    field("totalGoodResources"),
    field("totalWarningResources"),
    field("totalAlertedResources"),
    field("totalExpiredResources"),
    ShapeField {
        name: "extra",
        shape: EXTRA,
    },
]);

const ENVELOPE: Shape<'static> = Shape::Object(&[field("code"), field("data")]);
const ACCOUNT: Shape<'static> = Shape::Object(&[field("accountId"), field("userId")]);
const PUT: Shape<'static> = Shape::Object(&[field("successAll"), field("errorAutoRenewResources")]);

const fn field(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}

fn object(raw: &RawValue) -> bool {
    raw.get().starts_with('{')
}

pub(super) fn envelope(raw: &RawValue) -> Result<&RawValue, ()> {
    validate(raw, &ENVELOPE)?;
    #[derive(Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        code: &'a RawValue,
        #[serde(borrow)]
        data: &'a RawValue,
    }
    let envelope: Envelope<'_> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    if exact_integer(envelope.code.get()) != Some(200) || !object(envelope.data) {
        return Err(());
    }
    Ok(envelope.data)
}

pub(super) fn list(data: &RawValue) -> Result<(ListResourcesOutput, Vec<RenewPeriod>), ()> {
    validate(data, &DATA)?;
    let fields: BTreeMap<String, &RawValue> = serde_json::from_str(data.get()).map_err(|_| ())?;
    let rows: Vec<&RawValue> =
        serde_json::from_str(fields.get("data").ok_or(())?.get()).map_err(|_| ())?;
    if rows.iter().any(|row| !object(row)) {
        return Err(());
    }
    let projection: ListResourcesOutput = serde_json::from_str(data.get()).map_err(|_| ())?;
    let renew_periods = rows
        .into_iter()
        .zip(&projection.items)
        .map(|(raw, resource)| {
            if let Some(period) = resource.renew_period {
                return Ok(RenewPeriod::Value(period));
            }
            let fields: BTreeMap<String, &RawValue> =
                serde_json::from_str(raw.get()).map_err(|_| ())?;
            Ok(if fields.contains_key("renewPeriod") {
                RenewPeriod::Null
            } else {
                RenewPeriod::Omitted
            })
        })
        .collect::<Result<_, ()>>()?;
    Ok((projection, renew_periods))
}

pub(super) fn restore_raw_fields(
    source: &JsonSource<'_>,
    data: &RawValue,
    projection: &mut ListResourcesOutput,
) -> Result<(), ()> {
    let fields = object_fields(data)?;
    let rows: Vec<&RawValue> =
        serde_json::from_str(fields.get("data").ok_or(())?.get()).map_err(|_| ())?;
    for (raw, resource) in rows.into_iter().zip(&mut projection.items) {
        let row = object_fields(raw)?;
        resource.status = original_field(source, &row, "status")?;
        resource.status_ui = original_field(source, &row, "statusUI")?;
        if let Some(elements) = row
            .get("billingElements")
            .filter(|elements| elements.get() != "null")
        {
            let items: Vec<&RawValue> = serde_json::from_str(elements.get()).map_err(|_| ())?;
            for (raw, element) in items.into_iter().zip(&mut resource.billing_elements) {
                if raw.get() == "null" {
                    continue;
                }
                let fields = object_fields(raw)?;
                element.quantity = original_field(source, &fields, "quantity")?;
                element.meta_key = original_field(source, &fields, "metaKey")?;
            }
        }
    }
    if let Some(extra) = fields.get("extra").filter(|extra| extra.get() != "null") {
        let fields = object_fields(extra)?;
        projection.extra.warning_thresholds = original_field(source, &fields, "warningThresholds")?;
        projection.extra.alarm_thresholds = original_field(source, &fields, "alarmThresholds")?;
    }
    Ok(())
}

fn object_fields(raw: &RawValue) -> Result<BTreeMap<String, &RawValue>, ()> {
    serde_json::from_str(raw.get()).map_err(|_| ())
}

fn original_field(
    source: &JsonSource<'_>,
    fields: &BTreeMap<String, &RawValue>,
    key: &str,
) -> Result<Option<Box<RawValue>>, ()> {
    fields
        .get(key)
        .map(|raw| source.raw_fragment(raw).ok_or(()))
        .transpose()
}

pub(super) fn account(data: &RawValue) -> Result<String, ()> {
    validate(data, &ACCOUNT)?;
    let fields: BTreeMap<String, &RawValue> = serde_json::from_str(data.get()).map_err(|_| ())?;
    let mut account = None;
    for raw in [fields.get("accountId"), fields.get("userId")]
        .into_iter()
        .flatten()
    {
        let id = exact_integer(raw.get()).filter(|id| *id > 0).ok_or(())?;
        if account.replace(id).is_some_and(|previous| previous != id) {
            return Err(());
        }
    }
    account.map(|id| id.to_string()).ok_or(())
}

pub(super) fn renewal(data: &RawValue) -> Result<Option<usize>, ()> {
    validate(data, &PUT)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Renewal {
        success_all: bool,
        error_auto_renew_resources: Option<Vec<Box<RawValue>>>,
    }
    let output: Renewal = serde_json::from_str(data.get()).map_err(|_| ())?;
    let errors = output
        .error_auto_renew_resources
        .as_ref()
        .map_or(0, Vec::len);
    if !output.success_all || errors > 0 {
        return Ok(Some(errors));
    }
    output.error_auto_renew_resources.ok_or(())?;
    Ok(None)
}

fn validate(raw: &RawValue, schema: &Shape<'_>) -> Result<(), ()> {
    if !object(raw) || !exact_names(raw, schema) {
        return Err(());
    }
    strict_json::validate_shape(raw.get().as_bytes(), schema, raw.get().len()).map_err(|_| ())
}

fn exact_names(raw: &RawValue, shape: &Shape<'_>) -> bool {
    if raw.get() == "null" {
        return true;
    }
    match shape {
        Shape::Any => true,
        Shape::Object(fields) => serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get())
            .is_ok_and(|object| {
                object.iter().all(|(name, value)| {
                    fields
                        .iter()
                        .find(|field| strict_json::field_matches(field.name, name))
                        .is_none_or(|field| field.name == name && exact_names(value, &field.shape))
                })
            }),
        Shape::Array(item) => serde_json::from_str::<Vec<&RawValue>>(raw.get())
            .is_ok_and(|elements| elements.into_iter().all(|value| exact_names(value, item))),
    }
}

fn raw_value<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(decoder).map(Some)
}

fn serialize_raw<S: Serializer>(
    value: &Option<Box<RawValue>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => value.serialize(serializer),
        None => serializer.collect_seq(std::iter::empty::<()>()),
    }
}

fn billing_elements<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<ResourceBillingElement>, D::Error> {
    let elements: Vec<Option<ResourceBillingElement>> = null_default(decoder)?;
    Ok(elements
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

fn integer<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<i64>, D::Error> {
    let raw = Box::<RawValue>::deserialize(decoder)?;
    match raw.get() {
        "null" => Ok(None),
        "-0" => Ok(Some(0)),
        value => serde_json::from_str(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}
