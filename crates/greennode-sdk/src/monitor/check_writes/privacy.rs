use super::{Check, CheckRequest, Mutation, check_ids};
use crate::monitor::privacy::{check_budget, collect_url};
use crate::wire::product::evidence::Evidence;
use crate::{Error, Failure, Secret};
use serde_json::Value;

#[derive(Default)]
pub(super) struct Privacy {
    diagnostics: Evidence,
    operands: Evidence,
    exhausted: bool,
}
impl Privacy {
    pub(super) fn from_request(request: &CheckRequest) -> Self {
        let mut value = Self::default();
        value.add_request(request);
        value
    }
    pub(super) fn add_request(&mut self, request: &CheckRequest) {
        self.exhausted |= self.collect(request).is_err();
    }
    fn collect(&mut self, request: &CheckRequest) -> Result<(), ()> {
        add(&mut self.diagnostics, &request.url, true)?;
        add(&mut self.operands, &request.url, false)?;
        for (key, value) in request.headers.iter().chain(&request.query) {
            add(&mut self.diagnostics, value, true)?;
            add(&mut self.operands, value, is_credential_key(key))?;
        }
        add(&mut self.diagnostics, &request.body, true)?;
        add(&mut self.operands, &request.body, false)?;
        if serde_json::from_str::<&serde_json::value::RawValue>(&request.body).is_ok() {
            let json = serde_json::from_str::<Value>(&request.body).map_err(|_| ())?;
            visit(&mut self.diagnostics, &json, true)?;
            visit(&mut self.operands, &json, false)?;
        }
        Ok(())
    }
    pub(super) fn check_limit(
        &self,
        operation: &'static str,
        evidence: &Evidence,
    ) -> Result<(), Error> {
        if self.exhausted {
            return Err(crate::quote::request::monitor::privacy_limit(
                operation, evidence,
            ));
        }
        crate::monitor::mutation::privacy_credentials(operation, evidence, &self.diagnostics)
            .map(|_| ())
    }
    pub(super) fn diagnostics(&self) -> &Evidence {
        &self.diagnostics
    }
    pub(super) fn validate(
        &self,
        mutation: &Mutation,
        status: u16,
        check: &Check,
        evidence: &Evidence,
    ) -> Result<(), Error> {
        let invalid = || mutation.invalid(status, evidence);
        self.check_limit(mutation.id, evidence)?;
        if check_ids(check).any(|id| evidence.id_reflects(id) || self.operands.id_reflects(id)) {
            return Err(invalid());
        }
        let mut full = serde_json::to_value(check).map_err(|_| invalid())?;
        if reflects(&full, evidence) {
            return Err(invalid());
        }
        if let Some(request) = full
            .get_mut("Config")
            .and_then(|value| value.get_mut("Request"))
            .and_then(Value::as_object_mut)
        {
            request.retain(|key, _| key == "Method");
        }
        if reflects(&full, &self.operands) {
            return Err(invalid());
        }
        Ok(())
    }
    pub(super) fn screen_error(&self, error: Error) -> Error {
        crate::monitor::mutation::screen_details(error, &self.diagnostics)
    }
    pub(super) fn screen_failure<T>(&self, failure: Failure<T>) -> Failure<T> {
        let (error, partial, dispatch) = failure.into_parts();
        Failure::new(self.screen_error(error), partial, dispatch)
    }
    pub(super) fn withheld(&self, text: &str) -> bool {
        self.diagnostics.text_reflects(text)
    }
}
fn add(evidence: &mut Evidence, value: &str, sensitive: bool) -> Result<(), ()> {
    if evidence.credentials().values().len() > 256 {
        return Err(());
    }
    let pair = value.trim().split_once(' ');
    let scheme = pair.map_or("", |(scheme, _)| scheme);
    let auth = scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("Basic");
    if sensitive || auth {
        evidence.add_redaction(&Secret::new(value));
    }
    collect_url(evidence, value, |key| {
        sensitive || auth || is_credential_key(key)
    })?;
    if let Some((_, token)) = pair.filter(|_| auth) {
        evidence.add_redaction(&Secret::new(token.trim()));
        if scheme.eq_ignore_ascii_case("Basic")
            && let Ok(bytes) = data_encoding::BASE64.decode(token.trim().as_bytes())
            && let Ok(decoded) = String::from_utf8(bytes)
        {
            evidence.add_redaction(&Secret::new(&decoded));
            if let Some((left, right)) = decoded.split_once(':') {
                evidence.add_redaction(&Secret::new(left));
                evidence.add_redaction(&Secret::new(right));
            }
        }
    }
    check_budget(evidence)
}
fn visit(evidence: &mut Evidence, value: &Value, sensitive: bool) -> Result<(), ()> {
    if evidence.credentials().values().len() > 256 {
        return Err(());
    }
    match value {
        Value::String(text) => add(evidence, text, sensitive)?,
        Value::Number(number) if sensitive => {
            evidence.add_redaction(&Secret::new(number.to_string()))
        }
        Value::Array(items) => {
            for item in items {
                visit(evidence, item, sensitive)?;
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                visit(evidence, value, sensitive || is_credential_key(key))?;
            }
        }
        _ => (),
    }
    Ok(())
}
fn is_credential_key(key: &str) -> bool {
    let key: String = key
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_'))
        .collect();
    matches!(
        key.as_str(),
        "authorization"
            | "proxyauthorization"
            | "cookie"
            | "setcookie"
            | "key"
            | "passwd"
            | "pwd"
            | "otp"
            | "otpcode"
            | "otpref"
    ) || [
        "auth",
        "authentication",
        "authkey",
        "password",
        "secret",
        "secretkey",
        "token",
        "apikey",
        "accesskey",
        "privatekey",
        "credential",
        "credentials",
        "signature",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}
fn reflects(value: &Value, evidence: &Evidence) -> bool {
    use crate::output_number::{Style, f64};
    match value {
        Value::Number(number) => {
            evidence.text_reflects(number.as_str())
                || number.as_f64().is_none_or(|value| {
                    [Style::Json, Style::Fixed].into_iter().any(|style| {
                        f64(value, style).is_none_or(|text| evidence.text_reflects(&text))
                    })
                })
        }
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| evidence.text_reflects(key) || reflects(value, evidence)),
        Value::Array(items) => items.iter().any(|item| reflects(item, evidence)),
        _ => evidence.reflected_in(value),
    }
}
