use super::{Evidence, REDACTED, identifier_key, peer};
use serde_json::Value;

impl Evidence {
    pub(crate) fn serialized_output_reflects(&self, value: &impl serde::Serialize) -> bool {
        if !self.credentials.requires_screening() {
            return false;
        }
        super::super::identifier::checking(&self.credentials, || serde_json::to_vec(value))
            .map_or(true, |body| !peer::output_reflects(self, &body).is_clean())
    }

    pub(crate) fn output_reflects(&self, value: &Value) -> bool {
        self.output_value_reflects(value, false, false, &|_| false)
    }

    pub(crate) fn output_fields_reflect(&self, value: &Value) -> bool {
        self.output_fields_reflect_with(value, &|_| false)
    }

    pub(crate) fn identifier_output_reflects(&self, value: &Value) -> bool {
        self.output_value_reflects(value, true, true, &|_| false)
    }

    pub(crate) fn output_fields_reflect_with(
        &self,
        value: &Value,
        number_policy: &impl Fn(&serde_json::Number) -> bool,
    ) -> bool {
        self.output_value_reflects(value, false, true, number_policy)
    }

    fn output_value_reflects(
        &self,
        value: &Value,
        identifier: bool,
        immutable_only: bool,
        number_policy: &impl Fn(&serde_json::Number) -> bool,
    ) -> bool {
        match value {
            Value::Array(items) => items.iter().any(|item| {
                self.output_value_reflects(item, identifier, immutable_only, number_policy)
            }),
            Value::Object(map) => map.iter().any(|(key, item)| {
                (!immutable_only && self.text_reflects(key))
                    || (identifier && self.id_reflects(key))
                    || self.output_value_reflects(
                        item,
                        identifier || identifier_key(key),
                        immutable_only,
                        number_policy,
                    )
            }),
            Value::Number(number) if number_policy(number) => true,
            Value::String(_) | Value::Bool(_) | Value::Null if immutable_only && !identifier => {
                false
            }
            _ => self.output_scalar_reflects(value, identifier),
        }
    }

    pub(crate) fn redact(&self, value: &mut Value) -> bool {
        self.redact_value(value, false);
        !self.output_reflects(value)
    }

    pub(crate) fn redact_identifier_value(&self, value: &mut Value) -> bool {
        self.redact_value(value, true);
        !self.identifier_output_reflects(value)
    }

    fn redact_value(&self, value: &mut Value, identifier: bool) {
        match value {
            Value::Array(items) => items
                .iter_mut()
                .for_each(|item| self.redact_value(item, identifier)),
            Value::Object(map) => map.retain(|key, item| {
                if self.string_reflects(key, identifier) {
                    return false;
                }
                self.redact_value(item, identifier || identifier_key(key));
                true
            }),
            _ if self.output_scalar_reflects(value, identifier) => {
                *value = Value::String(REDACTED.to_owned());
            }
            _ => {}
        }
    }

    fn output_scalar_reflects(&self, value: &Value, identifier: bool) -> bool {
        match value {
            Value::String(text) => self.string_reflects(text, identifier),
            Value::Number(number) => {
                self.string_reflects(number.as_str(), identifier)
                    || number.as_f64().is_some_and(|amount| {
                        crate::output_number::any_spelling(amount, false, |text| {
                            self.string_reflects(text, identifier)
                        })
                    })
            }
            Value::Null => self.text_reflects("null"),
            Value::Bool(value) => self.text_reflects(if *value { "true" } else { "false" }),
            _ => false,
        }
    }
}
