use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};
use serde_json::Value;

use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;

use super::Policy;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PolicyProjection(Value);

impl PolicyProjection {
    pub(super) fn retain(policy: &mut Policy, evidence: &Evidence) -> Result<Option<Self>, ()> {
        if evidence.text_reflects("Policy") {
            return Err(());
        }
        let mut value = serde_json::to_value(&*policy).map_err(|_| ())?;
        if !evidence.output_reflects(&value) {
            return Ok(None);
        }
        if !evidence.redact(&mut value) {
            return Err(());
        }
        scrub_policy(policy, evidence);
        let typed = serde_json::to_value(&*policy).map_err(|_| ())?;
        if evidence.output_reflects(&typed) {
            return Err(());
        }
        Ok(Some(Self(value)))
    }
}

impl Serialize for PolicyProjection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Row(&self.0, POLICY).serialize(serializer)
    }
}

const POLICY: &[&str] = &[
    "UUID",
    "Name",
    "Description",
    "RedirectPoolID",
    "RedirectPoolName",
    "Action",
    "RedirectURL",
    "RedirectHTTPCode",
    "KeepQueryString",
    "Position",
    "L7Rules",
    "DisplayStatus",
    "CreatedAt",
    "UpdatedAt",
    "ProgressStatus",
];
const RULE: &[&str] = &[
    "UUID",
    "CompareType",
    "RuleValue",
    "RuleType",
    "ProvisioningStatus",
    "OperatingStatus",
];

struct Row<'a>(&'a Value, &'static [&'static str]);

impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(fields) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let mut row = serializer.serialize_struct("PolicyFields", fields.len())?;
        for &key in self.1 {
            if let Some(value) = fields.get(key) {
                match key {
                    "L7Rules" => row.serialize_field(key, &Rules(value))?,
                    _ => row.serialize_field(key, value)?,
                }
            }
        }
        row.end()
    }
}

struct Rules<'a>(&'a Value);

impl Serialize for Rules<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(serializer);
        };
        let mut sequence = serializer.serialize_seq(Some(items.len()))?;
        for item in items {
            sequence.serialize_element(&Row(item, RULE))?;
        }
        sequence.end()
    }
}

fn scrub_policy(policy: &mut Policy, evidence: &Evidence) {
    for text in [
        &mut policy.uuid,
        &mut policy.name,
        &mut policy.description,
        &mut policy.redirect_pool_id,
        &mut policy.redirect_pool_name,
        &mut policy.action,
        &mut policy.redirect_url,
        &mut policy.display_status,
        &mut policy.created_at,
        &mut policy.updated_at,
        &mut policy.progress_status,
    ] {
        text.redact(evidence);
    }
    for rule in &mut policy.l7_rules {
        for text in [
            &mut rule.uuid,
            &mut rule.compare_type,
            &mut rule.rule_value,
            &mut rule.rule_type,
            &mut rule.provisioning_status,
            &mut rule.operating_status,
        ] {
            text.redact(evidence);
        }
    }
    for number in [&mut policy.redirect_http_code, &mut policy.position] {
        if evidence.output_reflects(&Value::from(*number)) {
            *number = 0;
        }
    }
    if evidence.output_reflects(&Value::from(policy.keep_query_string)) {
        policy.keep_query_string = false;
    }
}
