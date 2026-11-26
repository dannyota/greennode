use super::sender::SentCredentials;
use crate::{Failure, Secret, WriteDispatch};
use serde_json::Value;

const REDACTED: &str = "<redacted>";

#[derive(Default)]
pub(crate) struct Evidence {
    credentials: SentCredentials,
}

impl Evidence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub(crate) fn add_secret(&mut self, secret: &Secret) {
        self.credentials.record(secret);
    }

    pub(crate) fn absorb(&mut self, credentials: &SentCredentials) {
        self.credentials.extend(credentials);
    }

    pub(crate) fn credentials(&self) -> &SentCredentials {
        &self.credentials
    }

    #[allow(dead_code)]
    pub(crate) fn reflected_in(&self, value: &Value) -> bool {
        match value {
            Value::String(text) => self.text_reflects(text),
            Value::Number(number) => self.number_reflects(number),
            Value::Array(items) => items.iter().any(|item| self.reflected_in(item)),
            Value::Object(map) => map
                .iter()
                .any(|(key, item)| self.text_reflects(key) || self.reflected_in(item)),
            Value::Null | Value::Bool(_) => false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn redact(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if self.text_reflects(text) {
                    *text = REDACTED.to_owned();
                }
            }
            Value::Number(number) => {
                if self.number_reflects(number) {
                    *value = Value::String(REDACTED.to_owned());
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.redact(item)),
            Value::Object(map) => {
                let mut redacted_entry = None;
                map.retain(|key, item| {
                    self.redact(item);
                    if self.text_reflects(key) {
                        redacted_entry = Some(std::mem::take(item));
                        false
                    } else {
                        true
                    }
                });
                if let Some(item) = redacted_entry {
                    map.entry(REDACTED).or_insert(item);
                }
            }
            Value::Null | Value::Bool(_) => {}
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_partial<T>(
        &self,
        failure: Failure<T>,
        mut partial: Value,
        dispatch: WriteDispatch,
    ) -> Failure<Value> {
        let (error, _, observed) = failure.into_parts();
        let dispatch = match (dispatch, observed) {
            (WriteDispatch::MayHaveBeenSent, _) | (_, WriteDispatch::MayHaveBeenSent) => {
                WriteDispatch::MayHaveBeenSent
            }
            (WriteDispatch::NotSent, _) | (_, WriteDispatch::NotSent) => WriteDispatch::NotSent,
            _ => WriteDispatch::NotAWrite,
        };
        self.redact(&mut partial);
        Failure::new(error, Some(partial), dispatch)
    }

    fn text_reflects(&self, text: &str) -> bool {
        self.credentials.reflects(text.as_bytes())
    }

    fn number_reflects(&self, number: &serde_json::Number) -> bool {
        self.credentials.reflects(number.as_str().as_bytes())
            || (may_reflect_rendered_number(&self.credentials)
                && number.as_f64().is_some_and(|value| {
                    crate::output_number::reflects(value, &self.credentials, false)
                }))
    }
}

pub(super) fn may_reflect_rendered_number(credentials: &SentCredentials) -> bool {
    credentials.values().iter().any(|secret| {
        secret
            .expose_secret()
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'.' | b'+' | b'-' | b'e'))
    })
}

impl std::fmt::Debug for Evidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Evidence([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evidence(secrets: &[&str]) -> Evidence {
        let mut evidence = Evidence::new();
        for secret in secrets {
            evidence.add_secret(&crate::Secret::new(*secret));
        }
        evidence
    }

    #[test]
    fn detects_secrets_in_strings_keys_and_nested_values() {
        let evidence = evidence(&["toy-secret"]);
        assert!(evidence.reflected_in(&json!({"a": ["x", {"b": "has toy-secret inside"}]})));
        assert!(evidence.reflected_in(&json!({"toy-secret": 1})));
        assert!(!evidence.reflected_in(&json!({"a": ["x", 1, null, true]})));
    }

    #[test]
    fn detects_rendered_numbers() {
        let evidence = evidence(&["1000000", "0.5"]);
        assert!(evidence.reflected_in(&json!({"n": 1e6})));
        assert!(evidence.reflected_in(&json!({"n": 0.5})));
        assert!(evidence.reflected_in(&json!([1000000])));
        assert!(!evidence.reflected_in(&json!([1000001, 0.25])));
    }

    #[test]
    fn redacts_only_reflecting_values() {
        let evidence = evidence(&["toy-secret", "42"]);
        let mut value = json!({"keep": "fine", "hit": "toy-secret", "n": 42, "m": 43, "k": {"toy-secret": "v"}});
        evidence.redact(&mut value);
        assert_eq!(
            value,
            json!({"keep": "fine", "hit": "<redacted>", "n": "<redacted>", "m": 43, "k": {"<redacted>": "v"}})
        );
        assert!(!evidence.reflected_in(&value));
    }

    #[test]
    fn secrets_added_later_apply_and_empty_secrets_are_ignored() {
        let mut evidence = evidence(&[""]);
        assert!(!evidence.reflected_in(&json!("anything")));
        evidence.add_secret(&crate::Secret::new("issued-password"));
        assert!(evidence.reflected_in(&json!("issued-password")));
        assert_eq!(format!("{evidence:?}"), "Evidence([redacted])");
    }
}
