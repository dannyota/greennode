use super::{Check, CheckRequest, Mutation, check_ids};
use crate::wire::product::evidence::Evidence;
use crate::{Error, Failure, Secret};
use serde_json::Value;

#[derive(Default)]
pub(super) struct Privacy {
    diagnostics: Evidence,
    operands: Evidence,
}
impl Privacy {
    pub(super) fn from_request(request: &CheckRequest) -> Self {
        let mut value = Self::default();
        value.add_request(request);
        value
    }
    pub(super) fn add_request(&mut self, request: &CheckRequest) {
        add(&mut self.diagnostics, &request.url, true);
        add(&mut self.operands, &request.url, false);
        for (key, value) in request.headers.iter().chain(&request.query) {
            add(&mut self.diagnostics, value, true);
            add(&mut self.operands, value, is_credential_key(key));
        }
        add(&mut self.diagnostics, &request.body, true);
        add(&mut self.operands, &request.body, false);
        if let Ok(json) = serde_json::from_str::<Value>(&request.body) {
            visit(&mut self.diagnostics, &json, true);
            visit(&mut self.operands, &json, false);
        }
    }
    pub(super) fn validate(
        &self,
        mutation: &Mutation,
        status: u16,
        check: &Check,
        evidence: &Evidence,
    ) -> Result<(), Error> {
        let invalid = || mutation.invalid(status, evidence);
        if evidence.credentials().values().len() > 256
            || self.diagnostics.credentials().values().len() > 256
            || self.operands.credentials().values().len() > 256
        {
            return Err(invalid());
        }
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
fn add(evidence: &mut Evidence, value: &str, sensitive: bool) {
    if evidence.credentials().values().len() > 256 {
        return;
    }
    let pair = value.trim().split_once(' ');
    let scheme = pair.map_or("", |(scheme, _)| scheme);
    let auth = scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("Basic");
    if sensitive || auth {
        evidence.add_redaction(&Secret::new(value));
    }
    if let Ok(url) = url::Url::parse(value) {
        evidence.add_redaction(&Secret::new(url.username()));
        if let Some(password) = url.password() {
            evidence.add_redaction(&Secret::new(password));
        }
        for (key, value) in url.query_pairs().take(257) {
            if sensitive || auth || is_credential_key(&key) {
                evidence.add_redaction(&Secret::new(value.into_owned()));
            }
        }
    }
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
}
fn visit(evidence: &mut Evidence, value: &Value, sensitive: bool) {
    if evidence.credentials().values().len() > 256 {
        return;
    }
    match value {
        Value::String(text) => add(evidence, text, sensitive),
        Value::Number(number) if sensitive => {
            evidence.add_redaction(&Secret::new(number.to_string()))
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| visit(evidence, item, sensitive)),
        Value::Object(fields) => fields
            .iter()
            .for_each(|(key, value)| visit(evidence, value, sensitive || is_credential_key(key))),
        _ => (),
    }
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
