mod peer;

use super::options::PasswordSlot;
use super::response::{self, Screen};
use crate::{Failure, Secret, WriteDispatch};
use serde_json::Value;
use std::sync::{Arc, LazyLock, OnceLock};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Source {
    Redaction,
    OperationPassword,
    Credential,
    Bearer,
}

#[derive(Clone, Default)]
pub(crate) struct SentCredentials {
    values: Vec<Secret>,
    sources: Vec<Source>,
    numeric_product_credential: bool,
    incomplete_signin: bool,
    patterns: Arc<OnceLock<Option<response::Patterns>>>,
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

    pub(crate) fn text_reflects(&self, text: &str) -> bool {
        response::reflects_credential(text, self)
    }

    pub(crate) fn id_reflects(&self, text: &str) -> bool {
        response::reflects_identifier(text, self)
    }

    pub(crate) fn record(&mut self, secret: &Secret) {
        self.record_from(secret, Source::Credential);
    }

    pub(crate) fn record_basic(&mut self, authorization: &str) {
        self.record(&Secret::new(authorization));
        if let Some(payload) = authorization.strip_prefix("Basic ") {
            self.record(&Secret::new(payload));
        }
    }

    pub(crate) fn record_bearer(&mut self, secret: &Secret) {
        self.record_from(secret, Source::Bearer);
    }

    pub(crate) fn bearers(&self) -> Vec<&Secret> {
        self.values
            .iter()
            .zip(&self.sources)
            .filter(|(_, source)| **source == Source::Bearer)
            .map(|(value, _)| value)
            .collect()
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
            self.patterns = Arc::default();
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
            patterns: Arc::default(),
        }
    }

    pub(super) fn record_product_credential(&mut self, secret: &Secret) {
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
        self.incomplete_signin |= credentials.incomplete_signin;
        self.numeric_product_credential |= credentials.numeric_product_credential;
        for (secret, source) in credentials
            .values
            .iter()
            .zip(credentials.sources.iter().copied())
        {
            self.record_from(secret, source);
        }
    }
    pub(crate) fn values(&self) -> &[Secret] {
        &self.values
    }
    pub(crate) fn integer_output_reflects(&self, value: i64) -> bool {
        response::reflects_credential(&value.to_string(), self)
            || crate::output_number::reflects(value as f64, self, false)
    }

    pub(super) fn patterns(&self) -> Option<&response::Patterns> {
        self.patterns
            .get_or_init(|| response::Patterns::compile(&self.values))
            .as_ref()
    }

    pub(crate) fn reflects(&self, data: &[u8]) -> bool {
        response::BodyMatcher::new(self, data.len()).raw(data)
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
    if body.is_empty() {
        return false;
    }
    let matcher = response::BodyMatcher::new(credentials, body.len());
    matcher.reflects(body, false) || peer::request_reflects(&matcher, body, false)
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
    let matcher = response::BodyMatcher::new(credentials, body.len());
    let reflects = |text: &str| matcher.reflects(text.as_bytes(), false);
    let other_credentials = credentials.without_operation_password(password);
    Some(
        (nested && reflects("user"))
            || reflects("password")
            || response::reflects_credential(authorized_field.get(), &other_credentials)
            || fields.into_iter().chain(user).any(|(name, value)| {
                let bytes = value.get().as_bytes();
                reflects(&name)
                    || matcher.reflects(bytes, false)
                    || peer::request_reflects(&matcher, bytes, peer::request_identifier_key(&name))
            }),
    )
}

pub(super) fn retained_projection_reflects(
    matcher: &response::BodyMatcher<'_>,
    body: &[u8],
) -> Screen {
    peer::output_with_matcher(matcher, body)
}

pub(super) fn identifier_projection_reflects(
    matcher: &response::BodyMatcher<'_>,
    body: &[u8],
) -> Screen {
    peer::output_identifier_reflects(matcher, body)
}

pub(crate) fn redact_identifier(value: &mut String, credentials: &SentCredentials) {
    if credentials.id_reflects(value) {
        *value = REDACTED.to_owned();
    }
}

const REDACTED: &str = "<redacted>";

const GUARD_BYTES: usize = 64 << 20;

pub(crate) type CachedCheck =
    std::sync::Arc<dyn Fn(&SentCredentials) -> Option<Box<crate::Error>> + Send + Sync>;

#[derive(Default)]
pub(crate) struct Evidence {
    credentials: SentCredentials,
    cached: Vec<CachedCheck>,
    guards: Vec<super::guard::Proof>,
    guard_bytes: usize,
    guard_scope: Option<Arc<std::sync::atomic::AtomicUsize>>,
    write_flow: bool,
    unproved_read: bool,
}

pub(crate) struct GuardReads(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for GuardReads {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Evidence {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn guard_reads(&mut self) -> GuardReads {
        let scope = self.guard_scope.get_or_insert_with(Arc::default);
        scope.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        GuardReads(Arc::clone(scope))
    }

    pub(crate) fn for_write() -> Self {
        Self {
            write_flow: true,
            ..Self::default()
        }
    }

    pub(super) fn guards_reads(&self) -> bool {
        self.write_flow
            || self
                .guard_scope
                .as_ref()
                .is_some_and(|scope| scope.load(std::sync::atomic::Ordering::Relaxed) != 0)
    }

    pub(crate) fn field_names(&self) -> crate::wire::strict_json::FieldNames {
        crate::wire::strict_json::FieldNames::for_guard(self.guards_reads())
    }

    pub(super) fn note_unproved_read(&mut self) {
        self.unproved_read = true;
    }

    pub(super) fn has_unproved_read(&self) -> bool {
        self.unproved_read
    }

    pub(crate) fn from_credentials(credentials: &SentCredentials) -> Self {
        Self {
            credentials: credentials.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn add_secret(&mut self, secret: &Secret) {
        self.credentials.record(secret);
        super::exit::record_secret(secret);
    }

    #[cfg(test)]
    pub(crate) fn add_bearer(&mut self, secret: &Secret) {
        self.credentials.record_bearer(secret);
    }

    pub(crate) fn add_redaction(&mut self, value: &Secret) {
        self.credentials.record_from(value, Source::Redaction);
        super::exit::record_secret(value);
    }

    pub(crate) fn add_operation_password(&mut self, value: &Secret) {
        self.credentials
            .record_from(value, Source::OperationPassword);
    }

    pub(crate) fn absorb(&mut self, credentials: &SentCredentials) {
        self.credentials.extend(credentials);
    }

    pub(crate) fn transport_only(&self) -> Self {
        Self {
            credentials: self.credentials.transport_only(),
            ..Self::default()
        }
    }

    pub(crate) fn without_operation_passwords(&self) -> Self {
        Self {
            credentials: self
                .credentials
                .filtered(|_, source| source != Source::OperationPassword),
            ..Self::default()
        }
    }

    pub(super) fn keep_guard(&mut self, id: &'static str, body: &[u8]) -> bool {
        let Some(total) = self.guard_bytes.checked_add(body.len()) else {
            return false;
        };
        if total > GUARD_BYTES || self.guards.len() == 512 {
            return false;
        }
        self.guard_bytes = total;
        self.guards.push(super::guard::Proof::new(id, body));
        true
    }

    #[cfg(test)]
    pub(crate) fn keep_guard_for_test(&mut self, id: &'static str, body: &[u8]) -> bool {
        self.keep_guard(id, body)
    }

    pub(crate) fn guard_checkpoint(&self) -> usize {
        self.guards.len()
    }

    pub(crate) fn discard_guards_after(&mut self, checkpoint: usize) {
        for proof in self.guards.drain(checkpoint..) {
            self.guard_bytes -= proof.retained_bytes();
        }
    }

    pub(super) fn guards_after(&self, checkpoint: usize) -> &[super::guard::Proof] {
        &self.guards[checkpoint..]
    }

    pub(super) fn last_guard(&self) -> Option<super::guard::Proof> {
        self.guards.last().cloned()
    }

    pub(crate) fn keep_decoded_operands(
        &mut self,
        operation: &'static str,
        value: &impl super::output::PublicOutput,
    ) -> Result<(), Box<crate::Error>> {
        let invalid = || Box::new(super::operation::unsafe_guard_response(operation));
        let proof = self
            .guards
            .last_mut()
            .filter(|proof| proof.belongs_to(operation))
            .ok_or_else(invalid)?;
        let fields = super::output::FieldScan::record(value, GUARD_BYTES - self.guard_bytes)
            .ok_or_else(invalid)?;
        self.guard_bytes += fields.bytes();
        proof.keep_operands(fields);
        Ok(())
    }

    pub(super) fn price_proof(
        &mut self,
        price: super::guard::DecisionPrice,
    ) -> Option<super::guard::Proof> {
        let proof = self.guards.last_mut()?;
        proof.attach_price(price).then(|| proof.clone())
    }

    pub(super) fn recheck_guards(
        &self,
        credentials: &SentCredentials,
    ) -> Option<Box<crate::Error>> {
        let transport = credentials.transport_only();
        let matcher = response::BodyMatcher::with_work(&transport, 192 << 20);
        self.guards.iter().find_map(|proof| proof.check(&matcher))
    }

    pub(crate) fn recheck_read_proofs(&self) -> Result<(), Box<crate::Error>> {
        let credentials = self.credentials();
        self.recheck_guards(credentials)
            .or_else(|| self.cached.iter().find_map(|check| check(credentials)))
            .map_or(Ok(()), Err)
    }

    pub(crate) fn keep_cached_check(&mut self, check: CachedCheck) {
        self.cached.push(check);
    }

    pub(crate) fn cached_checks(&self) -> &[CachedCheck] {
        &self.cached
    }

    pub(crate) fn credentials(&self) -> &SentCredentials {
        &self.credentials
    }

    pub(crate) fn can_retain_partial(&self) -> bool {
        !self.credentials.has_incomplete_signin()
    }

    pub(crate) fn serialized_output_reflects(&self, value: &impl serde::Serialize) -> bool {
        if self.credentials.values().is_empty() {
            return false;
        }
        super::identifier::checking(&self.credentials, || serde_json::to_vec(value))
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
        let partial = (self.can_retain_partial() && self.redact(&mut partial)).then_some(partial);
        Failure::new(self.screen_error(error), partial, dispatch)
    }

    pub(crate) fn text_reflects(&self, text: &str) -> bool {
        response::reflects_credential(text, &self.credentials)
    }

    pub(crate) fn id_reflects(&self, id: &str) -> bool {
        response::reflects_identifier(id, &self.credentials)
    }

    pub(crate) fn peer_bytes_reflect(&self, body: &[u8]) -> bool {
        response::reflects_bytes(body, &self.credentials)
    }

    pub(crate) fn peer_body_screen(&self, body: &[u8]) -> Screen {
        self.screen_peer_body(body, false)
    }

    pub(crate) fn go_peer_body_screen(&self, body: &[u8]) -> Screen {
        self.screen_peer_body(body, true)
    }

    fn screen_peer_body(&self, body: &[u8], go_compatible: bool) -> Screen {
        Self::screen_peer_with(
            &response::BodyMatcher::new(&self.credentials, body.len()),
            body,
            go_compatible,
        )
    }

    pub(super) fn screen_peer_with(
        matcher: &response::BodyMatcher<'_>,
        body: &[u8],
        go_compatible: bool,
    ) -> Screen {
        matcher.scan(body, false).then(|| {
            if body.iter().all(u8::is_ascii_whitespace) {
                return Screen::Clean;
            }
            if go_compatible {
                return peer::decoded_reflects(
                    matcher,
                    crate::model::wire::go_json(body).as_bytes(),
                );
            }
            peer::decoded_reflects(matcher, body)
        })
    }

    pub(crate) fn output_number_reflects(&self, amount: f64, include_sdk_json: bool) -> bool {
        crate::output_number::reflects(amount, &self.credentials, include_sdk_json)
    }

    pub(crate) fn integer_output_reflects(&self, value: i64) -> bool {
        self.credentials.integer_output_reflects(value)
    }

    pub(crate) fn reflects_fixed_two(&self, amount: f64) -> bool {
        !amount.is_finite() || self.text_reflects(&crate::output_number::fixed_two(amount))
    }

    pub(crate) fn screen_error(&self, error: crate::Error) -> crate::Error {
        error.screen_api(&|text| self.text_reflects(text))
    }

    pub(crate) fn screen_public_error(&self, error: crate::Error) -> crate::Error {
        error.screen_public(&|text| self.text_reflects(text))
    }

    fn string_reflects(&self, text: &str, identifier: bool) -> bool {
        if identifier {
            self.id_reflects(text)
        } else {
            self.text_reflects(text)
        }
    }
}

pub(crate) fn identifier_key(key: &str) -> bool {
    let mut start = 0;
    let mut previous = None;
    for (index, ch) in key.char_indices() {
        if matches!(ch, '_' | '-') {
            start = index + 1;
        } else if ch.is_uppercase() && previous.is_some_and(char::is_lowercase) {
            start = index;
        }
        previous = Some(ch);
    }
    let suffix = &key[start..];
    ["id", "ids", "uuid", "uuids"]
        .into_iter()
        .any(|word| suffix.eq_ignore_ascii_case(word))
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
    fn literal_byte_scan_keeps_its_rules_and_refuses_uncompiled_credentials() {
        let exact = SentCredentials::for_test(vec![crate::Secret::new("Toy-A\nlong-line")]);
        assert!(exact.reflects(b"\xffToy-A\nlong-line\xff"));
        assert!(!exact.reflects(b"toy-a\nlong-line"));
        assert!(!exact.reflects(b"long-line"));
        let oversized = SentCredentials::for_test(vec![crate::Secret::new("x".repeat(64 << 10))]);
        assert!(oversized.reflects(b"ordinary"));
    }

    #[test]
    fn outgoing_json_with_exhausted_depth_is_refused() {
        let evidence = evidence(&["jkl"]);
        let body = format!(r#"{{"value":{}0{}}}"#, "[".repeat(140), "]".repeat(140));
        assert!(body_reflects(body.as_bytes(), evidence.credentials(), None));
    }

    #[test]
    fn write_body_strictly_checks_identifier_values() {
        let evidence = evidence(&["jkl"]);
        let credentials = evidence.credentials();
        assert!(!body_reflects(
            br#"{"note":"pre6A6B6Cpost"}"#,
            credentials,
            None
        ));
        assert!(body_reflects(
            br#"{"assocVpcIds":["pre6A6B6Cpost"]}"#,
            credentials,
            None
        ));
    }

    #[test]
    fn assembled_sdk_json_screens_a_credential_across_syntax_after_decoding() {
        let evidence = evidence(&[r#"note":"toy secret"#]);
        assert!(evidence.serialized_output_reflects(&json!({"note": "toy%20secret"})));
    }

    #[test]
    fn detects_secrets_in_strings_keys_and_nested_values() {
        let evidence = evidence(&["toy-secret"]);
        assert!(evidence.output_reflects(&json!({"a": ["x", {"b": "has toy-secret inside"}]})));
        assert!(evidence.output_reflects(&json!({"toy-secret": 1})));
        assert!(!evidence.output_reflects(&json!({"a": ["x", 1, null, true]})));
    }

    #[test]
    fn clean_short_base64_field_does_not_reflect_a_byte() {
        let evidence = evidence(&["a"]);
        assert_eq!(
            evidence.peer_body_screen(br#"{"note":"QmFk"}"#),
            Screen::Clean
        );
        assert_eq!(
            evidence.peer_body_screen(br#"{"note":"YQ=="}"#),
            Screen::Reflected
        );
    }

    #[test]
    fn short_credential_does_not_reflect_inside_a_longer_base64_field() {
        for evidence in [evidence(&["a"]), evidence(&["a", "long-toy-secret"])] {
            assert_eq!(
                evidence.peer_body_screen(br#"{"note":"QmFkZ2Vy"}"#),
                Screen::Clean
            );
            assert_eq!(
                evidence.peer_body_screen(br#"{"note":"YQ=="}"#),
                Screen::Reflected
            );
        }
    }

    #[test]
    fn peer_body_charges_all_decoded_fields_to_one_budget() {
        let evidence = Evidence::from_credentials(&response::budget_test_credentials());
        let field = format!("!{}", "a".repeat(255));
        let single = serde_json::to_vec(&[field.as_str()]).unwrap();
        let page = serde_json::to_vec(&vec![field; 1024]).unwrap();
        assert_eq!(evidence.peer_body_screen(&single), Screen::Clean);
        assert!(!evidence.peer_bytes_reflect(&page));
        assert_eq!(evidence.peer_body_screen(&page), Screen::Indeterminate);
    }

    #[test]
    fn redaction_agrees_with_refusal_inside_identifier_objects() {
        let evidence = evidence(&["jkl"]);
        let mut value = json!({
            "projectId": {"name": "pre6A6B6Cpost", "pre6A6B6Cpost": true},
            "ordinary": {"name": "pre6A6B6Cpost"}
        });
        assert!(evidence.output_reflects(&value));
        evidence.redact(&mut value);
        assert!(!evidence.output_reflects(&value));
        assert!(
            value
                == json!({"projectId": {"name": "<redacted>"}, "ordinary": {"name": "pre6A6B6Cpost"}})
        );
    }

    #[test]
    fn detects_rendered_numbers() {
        let evidence = evidence(&["1000000", "0.5"]);
        assert!(evidence.output_reflects(&json!({"n": 1e6})));
        assert!(evidence.output_reflects(&json!({"n": 0.5})));
        assert!(evidence.output_reflects(&json!([1000000])));
        assert!(!evidence.output_reflects(&json!([1000001, 0.25])));
    }

    #[test]
    fn oversized_multiline_credential_fails_closed_during_pattern_construction() {
        let long = (0..600)
            .map(|index| format!("synthetic-{index:04}-segment"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(evidence(&[&long]).text_reflects("ordinary output"));
        assert!(
            !evidence(&["synthetic-0000-segment\nsynthetic-0001-segment"])
                .text_reflects("ordinary output")
        );
    }

    #[test]
    fn numeric_values_allow_a_reflection_in_unused_go_map_key_spelling() {
        let evidence = evidence(&["e+06"]);
        assert!(!evidence.output_number_reflects(1e6, true));
        assert!(!evidence.output_reflects(&json!({"price": 1e6})));
    }

    #[test]
    fn numeric_ids_use_identifier_matching_in_peer_and_retained_values() {
        let evidence = evidence(&["a"]);
        assert_eq!(
            evidence.peer_body_screen(br#"{"projectId":9619}"#),
            Screen::Reflected
        );
        assert!(evidence.output_reflects(&json!({"projectId": 9619})));
        assert!(!evidence.output_reflects(&json!({"count": 9619})));
    }

    #[test]
    fn output_integer_screens_exact_and_rounded_spellings() {
        assert!(evidence(&["00000009"]).output_reflects(&json!(3030303030303039_i64)));
        assert!(evidence(&["9007199254740993"]).output_reflects(&json!(9007199254740993_i64)));
        assert!(evidence(&["9007199254740992"]).output_reflects(&json!(9007199254740993_i64)));
    }

    #[test]
    fn kept_price_screens_its_two_decimal_spelling() {
        let evidence = evidence(&["P"]);
        assert!(evidence.reflects_fixed_two(49.999));
        assert!(!evidence.output_reflects(&json!({"amount": 49.999})));
        assert!(!evidence.reflects_fixed_two(51.0));
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
        assert!(!evidence.output_reflects(&value));
    }

    #[test]
    fn secrets_added_later_apply_and_empty_secrets_are_ignored() {
        let mut evidence = evidence(&[""]);
        assert!(!evidence.output_reflects(&json!("anything")));
        evidence.add_secret(&crate::Secret::new("issued-password"));
        assert!(evidence.output_reflects(&json!("issued-password")));
        assert_eq!(format!("{evidence:?}"), "Evidence([redacted])");
    }
}
