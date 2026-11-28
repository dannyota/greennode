use std::collections::BTreeMap;

use serde_json::{Number, value::RawValue};
use zeroize::Zeroizing;

use crate::wire::product::{evidence::Evidence, operation};
use crate::{model::wire::go_json, output_number};

use super::ListResourcesOutput;

pub(super) fn reflected(raw: &RawValue, evidence: &Evidence) -> bool {
    evidence.credentials().has_numeric_product_credential()
        || operation::reflects_json(raw.get().as_bytes(), evidence.credentials(), true)
            .unwrap_or(true)
        || token_reflects(raw, evidence, false)
}

fn token_reflects(raw: &RawValue, evidence: &Evidence, identifier: bool) -> bool {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => {
            serde_json::from_str::<BTreeMap<String, &RawValue>>(text).map_or(true, |fields| {
                fields.into_iter().any(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let identifier = lower == "id"
                        || lower == "uuid"
                        || lower.ends_with("_id")
                        || lower.ends_with("_ids")
                        || ["Id", "ID", "Ids", "IDs", "Uuid", "UUID"]
                            .into_iter()
                            .any(|suffix| key.ends_with(suffix));
                    evidence.text_reflects(&key) || token_reflects(value, evidence, identifier)
                })
            })
        }
        Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(text).map_or(true, |items| {
            items
                .into_iter()
                .any(|item| token_reflects(item, evidence, identifier))
        }),
        Some(b'"') => serde_json::from_str::<String>(text).map_or(true, |value| {
            let value = Zeroizing::new(value);
            if identifier {
                evidence.id_reflects(&value)
            } else {
                evidence.text_reflects(&value)
            }
        }),
        Some(b'-' | b'0'..=b'9') => {
            let matches = |value: &str| {
                if identifier {
                    evidence.id_reflects(value)
                } else {
                    evidence.text_reflects(value)
                }
            };
            matches(text)
                || serde_json::from_str::<Number>(text).map_or(true, |number| {
                    number
                        .as_f64()
                        .is_some_and(|value| output_number::rendered_matches(value, matches))
                })
        }
        _ => evidence.text_reflects(text),
    }
}

pub(super) fn resources_reflect(output: &ListResourcesOutput, evidence: &Evidence) -> bool {
    let integer = |value: Option<i64>| {
        value.map_or_else(
            || evidence.text_reflects("null"),
            |value| {
                evidence.text_reflects(&value.to_string())
                    || output_number::rendered_matches(value as f64, |text| {
                        evidence.text_reflects(text)
                    })
            },
        )
    };
    let raw = |value: &Option<Box<RawValue>>| {
        value.as_ref().is_some_and(|value| {
            let text = go_json(value.get().as_bytes());
            serde_json::from_str::<&RawValue>(&text)
                .map_or(true, |raw| token_reflects(raw, evidence, false))
        })
    };
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
                    |cost| {
                        output_number::rendered_matches(cost, |text| evidence.text_reflects(text))
                    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Secret;

    #[test]
    fn recognizes_identifier_keys_and_numeric_identifiers() {
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new("012"));
        for key in ["uuid", "project_id", "other_ids", "otherId", "otherIds"] {
            for value in ["13031329", "\"13031329\""] {
                let text = format!("{{\"{key}\":{value}}}");
                let raw: &RawValue = serde_json::from_str(&text).unwrap();
                assert!(token_reflects(raw, &evidence, false), "key {key}");
            }
        }
    }
}
