use serde_json::value::RawValue;

use crate::wire::product::{
    evidence::Evidence,
    output::{FieldScan, PublicOutput},
};

use super::ListResourcesOutput;

pub(super) fn resources_reflect(output: &ListResourcesOutput, evidence: &Evidence) -> bool {
    let scan = FieldScan::new(evidence);
    let integer = |value: Option<i64>| {
        value.map_or_else(
            || evidence.text_reflects("null"),
            |value| evidence.integer_output_reflects(value),
        )
    };
    let raw = |value: &Option<Box<RawValue>>| value.public_reflects(&scan, false);
    [
        output.total_auto_renews,
        output.total_good_resources,
        output.total_warning_resources,
        output.total_alerted_resources,
        output.total_expired_resources,
    ]
    .into_iter()
    .any(integer)
        || raw(&output.extra.warning_thresholds)
        || raw(&output.extra.alarm_thresholds)
        || output.items.iter().any(|row| {
            evidence.id_reflects(&row.artifact_id)
                || [
                    &row.artifact_name,
                    &row.artifact_type,
                    &row.product,
                    &row.renew_type,
                    &row.billing_type,
                ]
                .into_iter()
                .any(|value| evidence.text_reflects(value))
                || [
                    row.start_billing_time,
                    row.end_billing_time,
                    row.date_left,
                    row.renew_period,
                    row.channel,
                ]
                .into_iter()
                .any(integer)
                || evidence.text_reflects(match row.is_renewing {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "null",
                })
                || row.cost.map_or_else(
                    || evidence.text_reflects("null"),
                    |cost| evidence.output_number_reflects(cost, true),
                )
                || raw(&row.status)
                || raw(&row.status_ui)
                || row.billing_elements.iter().any(|element| {
                    evidence.text_reflects(&element.sku)
                        || raw(&element.quantity)
                        || raw(&element.meta_key)
                })
        })
}
