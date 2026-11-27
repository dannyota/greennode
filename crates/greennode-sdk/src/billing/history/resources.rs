use super::{
    Envelope, failure, history_operation,
    security::{RESOURCE_WITHHELD, response_error, status_error},
};
use crate::billing::{
    Client,
    model::{exact_integer, field, null_default},
};
use crate::wire::{
    product::{
        evidence::Evidence,
        operation::{self, Operation, Output},
        sender::Response,
    },
    strict_json::{self, Shape, ShapeField},
};
use crate::{Error, Failure, RequestContext};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

/// Billing resource cannot be renewed.
pub const RENEW_TYPE_NON_RENEWABLE: &str = "NON-RENEWABLE";
/// Billing resource renews only by request.
pub const RENEW_TYPE_MANUAL: &str = "MANUAL";
/// Billing resource is set to renew automatically.
pub const RENEW_TYPE_AUTO_RENEW: &str = "AUTO-RENEW";

/// Reads prepaid resources across products without filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListResourcesInput {}

/// Prepaid resource renewal state and aggregate counts.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase", serialize = "PascalCase"))]
pub struct ListResourcesOutput {
    /// Resources in API order.
    #[serde(rename(deserialize = "data", serialize = "Items"))]
    pub items: Vec<Resource>,
    /// Resources with auto-renew enabled, when reported.
    pub total_auto_renews: Option<i64>,
    /// Resources in good standing, when reported.
    pub total_good_resources: Option<i64>,
    /// Resources in a warning state, when reported.
    pub total_warning_resources: Option<i64>,
    /// Resources in an alert state, when reported.
    pub total_alerted_resources: Option<i64>,
    /// Expired resources, when reported.
    pub total_expired_resources: Option<i64>,
    /// Threshold data as reported by billing.
    #[serde(default, deserialize_with = "null_default")]
    pub extra: ResourceThresholds,
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

/// A prepaid resource's identity, renewal state, and billing details.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    pub start_billing_time: Option<i64>,
    /// Billing end time in epoch milliseconds, when reported.
    pub end_billing_time: Option<i64>,
    /// Days remaining, when reported.
    pub date_left: Option<i64>,
    /// Renewal period, when reported.
    pub renew_period: Option<i64>,
    /// Billing channel, when reported.
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

/// One billing element of a prepaid resource.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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

crate::wire::product::output::public_fields!(ResourceBillingElement {
    sku,
    quantity,
    meta_key
});

/// Warning and alarm thresholds supplied with prepaid resources.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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

crate::wire::product::output::public_fields!(ResourceThresholds {
    warning_thresholds,
    alarm_thresholds
});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads account-wide prepaid renewal state.
    pub async fn list_resources(
        &self,
        ctx: &RequestContext,
        _input: ListResourcesInput,
    ) -> Result<ListResourcesOutput, Failure<ListResourcesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = Operation {
                cap: 4 << 20,
                reflection: crate::wire::product::operation::Reflection::Refuse,
                overflow_overrides_status: true,
                ..history_operation("billing.ListResources", "gateway/api/v1/resources")
            };
            self.history_request::<ResourceEnvelope>(ctx, &operation, &[], RESOURCE_WITHHELD)
                .await
                .map(|output| output.data)
                .map_err(|error| failure(*error))
        })
    }
}

#[derive(Default, Deserialize)]
struct ResourceEnvelope {
    data: ListResourcesOutput,
}

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

impl Output for ResourceEnvelope {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("code"),
        ShapeField {
            name: "data",
            shape: DATA,
        },
    ]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let status = response.status;
        let invalid = || {
            Box::new(response_error(
                operation,
                Some(status),
                "InvalidResponse",
                RESOURCE_WITHHELD,
                false,
            ))
        };
        if !operation.success.contains(&status) {
            return Err(Box::new(status_error(operation, status, RESOURCE_WITHHELD)));
        }
        let raw: &RawValue = serde_json::from_slice(&response.body).map_err(|_| invalid())?;
        if response.credentials.has_numeric_product_credential() || !exact_names(raw, &Self::SCHEMA)
        {
            return Err(invalid());
        }
        let envelope: Envelope<'_> = serde_json::from_str(raw.get()).map_err(|_| invalid())?;
        if exact_integer(envelope.code.get()) != Some(200) {
            return Err(invalid());
        }
        let output: Self =
            operation::decode_response(operation, response).map_err(|_| invalid())?;
        let evidence = Evidence::from_credentials(&response.credentials);
        if serde_json::to_value(&output.data).map_or(true, |value| evidence.output_reflects(&value))
        {
            return Err(invalid());
        }
        Ok(output)
    }
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
