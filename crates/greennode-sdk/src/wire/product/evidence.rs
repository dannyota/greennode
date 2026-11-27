use super::options::PasswordSlot;
use super::response;
use crate::{Failure, Secret, WriteDispatch};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Source {
    Redaction,
    OperationPassword,
    Credential,
}

#[derive(Clone, Default)]
pub(crate) struct SentCredentials {
    values: Vec<Secret>,
    sources: Vec<Source>,
    numeric_product_credential: bool,
    incomplete_signin: bool,
    discarded_credentials: bool,
    observed_product_credential: bool,
}
impl SentCredentials {
    #[cfg(test)]
    pub(crate) fn for_test(values: Vec<Secret>) -> Self {
        Self {
            sources: vec![Source::Credential; values.len()],
            values,
            ..Self::default()
        }
    }

    pub(crate) fn record(&mut self, secret: &Secret) {
        self.record_from(secret, Source::Credential);
    }

    fn record_from(&mut self, secret: &Secret, source: Source) {
        if secret.expose_secret().is_empty() {
            return;
        }
        if let Some((_, known_source)) = self
            .values
            .iter()
            .zip(&mut self.sources)
            .find(|(value, _)| value.expose_secret() == secret.expose_secret())
        {
            *known_source = (*known_source).max(source);
        } else {
            self.values.push(secret.clone());
            self.sources.push(source);
        }
    }

    pub(super) fn transport_only(&self) -> Self {
        if self.sources.contains(&Source::Redaction) {
            self.filtered(|_, source| source != Source::Redaction)
        } else {
            self.clone()
        }
    }

    fn without_operation_password(&self, password: &Secret) -> Self {
        self.filtered(|value, source| {
            source != Source::OperationPassword || value.expose_secret() != password.expose_secret()
        })
    }

    fn filtered(&self, keep: impl Fn(&Secret, Source) -> bool) -> Self {
        let (values, sources) = self
            .values
            .iter()
            .zip(self.sources.iter().copied())
            .filter(|(value, source)| keep(value, *source))
            .map(|(value, source)| (value.clone(), source))
            .unzip();
        Self {
            values,
            sources,
            numeric_product_credential: self.numeric_product_credential,
            incomplete_signin: self.incomplete_signin,
            discarded_credentials: self.discarded_credentials,
            observed_product_credential: self.observed_product_credential,
        }
    }

    pub(super) fn record_product_credential(&mut self, secret: &Secret) {
        self.observed_product_credential = true;
        static NUMBER: LazyLock<Result<regex_lite::Regex, regex_lite::Error>> =
            LazyLock::new(|| {
                regex_lite::Regex::new(r"^[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$")
            });
        self.numeric_product_credential |= NUMBER.as_ref().map_or(true, |pattern| {
            pattern.is_match(secret.expose_secret().trim())
        });
        self.record(secret);
    }
    pub(crate) fn has_numeric_product_credential(&self) -> bool {
        self.numeric_product_credential
    }
    pub(super) fn has_incomplete_signin(&self) -> bool {
        self.incomplete_signin
    }
    pub(super) fn mark_incomplete_signin(&mut self) {
        self.incomplete_signin = true;
    }

    pub(crate) fn extend(&mut self, credentials: &Self) {
        self.extend_bounded(credentials, usize::MAX);
    }

    fn extend_bounded(&mut self, credentials: &Self, limit: usize) -> bool {
        self.incomplete_signin |= credentials.incomplete_signin;
        self.discarded_credentials |= credentials.discarded_credentials;
        self.observed_product_credential |= credentials.observed_product_credential;
        self.numeric_product_credential |= credentials.numeric_product_credential;
        let mut complete = true;
        for (secret, source) in credentials
            .values
            .iter()
            .zip(credentials.sources.iter().copied())
        {
            if self.values.len() >= limit
                && !self
                    .values
                    .iter()
                    .any(|known| known.expose_secret() == secret.expose_secret())
            {
                complete = false;
            } else {
                self.record_from(secret, source);
            }
        }
        complete
    }
    pub(crate) fn values(&self) -> &[Secret] {
        &self.values
    }
    pub(crate) fn reflects(&self, data: &[u8]) -> bool {
        self.values.iter().any(|secret| {
            let value = secret.expose_secret().as_bytes();
            !value.is_empty() && data.windows(value.len()).any(|part| part == value)
        })
    }
}
impl std::fmt::Debug for SentCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SentCredentials([redacted])")
    }
}
pub(super) fn body_reflects(
    body: &[u8],
    credentials: &SentCredentials,
    password_slot: Option<PasswordSlot<'_>>,
) -> bool {
    if let Some(slot) = password_slot {
        return check_password_body(body, credentials, slot).unwrap_or(true);
    }
    !body.is_empty() && response::reflects_credential(&String::from_utf8_lossy(body), credentials)
}

fn check_password_body(
    body: &[u8],
    credentials: &SentCredentials,
    slot: PasswordSlot<'_>,
) -> Option<bool> {
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;
    type Fields<'a> = BTreeMap<String, &'a RawValue>;

    crate::wire::strict_json::validate(body, &[], body.len()).ok()?;
    let mut fields: Fields<'_> = serde_json::from_slice(body).ok()?;
    let mut user = Fields::new();
    let (authorized_field, password, nested) = match slot {
        PasswordSlot::DatabaseUser(password) => {
            user = serde_json::from_str(fields.remove("user")?.get()).ok()?;
            (user.remove("password")?, password, true)
        }
        PasswordSlot::IamUser(password) => (fields.remove("password")?, password, false),
    };
    let authorized_value =
        zeroize::Zeroizing::new(serde_json::from_str::<String>(authorized_field.get()).ok()?);
    if authorized_value.as_str() != password.expose_secret() {
        return None;
    }
    let reflects = |text: &str| response::reflects_credential(text, credentials);
    let other_credentials = credentials.without_operation_password(password);
    Some(
        (nested && reflects("user"))
            || reflects("password")
            || response::reflects_credential(authorized_field.get(), &other_credentials)
            || fields
                .into_iter()
                .chain(user)
                .any(|(name, value)| reflects(&name) || reflects(value.get())),
    )
}

const REDACTED: &str = "<redacted>";

#[derive(Default)]
pub(crate) struct Evidence {
    credentials: SentCredentials,
}

impl Evidence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add_secret(&mut self, secret: &Secret) {
        self.credentials.record(secret);
    }

    pub(crate) fn add_redaction(&mut self, value: &Secret) {
        self.credentials.record_from(value, Source::Redaction);
    }

    pub(crate) fn add_operation_password(&mut self, value: &Secret) {
        self.credentials
            .record_from(value, Source::OperationPassword);
    }

    pub(crate) fn absorb(&mut self, credentials: &SentCredentials) {
        self.credentials.extend(credentials);
    }

    pub(crate) fn absorb_bounded(&mut self, credentials: &SentCredentials, limit: usize) -> bool {
        self.credentials.extend_bounded(credentials, limit)
    }

    pub(crate) fn mark_discarded_credentials(&mut self) {
        self.credentials.discarded_credentials = true;
    }

    pub(crate) fn for_first_login_preflight(&self) -> Option<Self> {
        let credentials = &self.credentials;
        if !credentials.incomplete_signin
            || credentials.discarded_credentials
            || credentials.observed_product_credential
        {
            return None;
        }
        let mut credentials = credentials.clone();
        credentials.incomplete_signin = false;
        Some(Self { credentials })
    }

    pub(crate) fn transport_only(&self) -> Self {
        Self {
            credentials: self.credentials.transport_only(),
        }
    }

    pub(crate) fn without_operation_passwords(&self) -> Self {
        Self {
            credentials: self
                .credentials
                .filtered(|_, source| source != Source::OperationPassword),
        }
    }

    pub(crate) fn credentials(&self) -> &SentCredentials {
        &self.credentials
    }

    pub(crate) fn can_retain_partial(&self) -> bool {
        !self.credentials.has_incomplete_signin() && !self.credentials.discarded_credentials
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
            Value::Null | Value::Bool(_) => self.literal_reflects(value),
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
                map.retain(|key, item| {
                    if self.text_reflects(key) {
                        return false;
                    }
                    self.redact(item);
                    true
                });
            }
            Value::Null | Value::Bool(_) => {
                if self.literal_reflects(value) {
                    *value = Value::String(REDACTED.to_owned());
                }
            }
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
        let partial = self.can_retain_partial().then(|| {
            self.redact(&mut partial);
            partial
        });
        Failure::new(error, partial, dispatch)
    }

    pub(crate) fn text_reflects(&self, text: &str) -> bool {
        response::reflects_credential(text, &self.credentials)
    }

    pub(crate) fn id_reflects(&self, id: &str) -> bool {
        response::reflects_identifier(id, &self.credentials)
    }

    pub(crate) fn reflects_fixed_two(&self, amount: f64) -> bool {
        may_reflect_rendered_number(&self.credentials)
            && self
                .credentials
                .reflects(crate::output_number::fixed_two(amount).as_bytes())
    }

    fn literal_reflects(&self, value: &Value) -> bool {
        let text: &[u8] = match value {
            Value::Null => b"null",
            Value::Bool(true) => b"true",
            Value::Bool(false) => b"false",
            _ => return false,
        };
        self.credentials.reflects(text)
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
        let mut value = json!({"keep": "fine", "hit": "toy-secret", "n": 42, "m": 43, "k": {"toy-secret": "drop", "keep": "stay"}});
        evidence.redact(&mut value);
        assert_eq!(
            value,
            json!({"keep": "fine", "hit": "<redacted>", "n": "<redacted>", "m": 43, "k": {"keep": "stay"}})
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
