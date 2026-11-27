mod encoded;
mod full_error;
mod marker;
mod patterns;

use super::{redirect::Retry, sender::SentCredentials};
use bytes::Bytes;
use encoded::EncodedNeedle;
use full_error::full_error;
pub(crate) use full_error::inspect_error_message;
use http::StatusCode;
pub(crate) use marker::has_credential_marker;
pub(super) use patterns::Patterns;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use zeroize::Zeroizing;

pub(crate) fn classify(
    operation: &'static str,
    status: StatusCode,
    body: Bytes,
    success: &[StatusCode],
    rules: (bool, bool, Retry),
    credentials: &SentCredentials,
) -> Result<Bytes, Box<crate::Error>> {
    classify_with_decoder(
        operation,
        status,
        body,
        success,
        rules,
        credentials,
        |body| decode(body, !credentials.values().is_empty()),
    )
}

pub(crate) fn classify_full(
    operation: &'static str,
    status: StatusCode,
    body: Bytes,
    success: &[StatusCode],
    rules: (bool, bool, Retry),
    credentials: &SentCredentials,
) -> Result<Bytes, Box<crate::Error>> {
    classify_with_decoder(
        operation,
        status,
        body,
        success,
        rules,
        credentials,
        |body| full_error(body).unwrap_or_default(),
    )
}

fn classify_with_decoder(
    operation: &'static str,
    status: StatusCode,
    body: Bytes,
    success: &[StatusCode],
    rules: (bool, bool, Retry),
    credentials: &SentCredentials,
    decode: impl FnOnce(&[u8]) -> (String, String),
) -> Result<Bytes, Box<crate::Error>> {
    if success.contains(&status) {
        return Ok(body);
    }
    let (fallback, sensitive, retry) = rules;
    let (code, message) = if sensitive {
        (String::new(), String::new())
    } else {
        decode(&body)
    };
    let secrets: Vec<_> = credentials.values().iter().collect();
    let safe = |text: &str| sanitize_error_text(text, credentials);
    let fallback = fallback
        || contains_address_marker(&code)
        || contains_address_marker(crate::error::SafeText::new(&code, &secrets).as_str());
    let code = safe(&code);
    let message = if sensitive
        || message.is_empty()
        || (!secrets.is_empty() && unsafe_message(&message, credentials))
    {
        safe(status_text(status))
    } else {
        let value = safe(message.trim());
        if value.as_str().is_empty() {
            safe(status_text(status))
        } else {
            value
        }
    };
    let retryable = retryable(status, retry);
    let api = if fallback {
        crate::ApiError::new(operation, Some(status.as_u16()), code, message, retryable)
    } else {
        crate::ApiError::without_fallback(
            operation,
            Some(status.as_u16()),
            code,
            message,
            retryable,
        )
    };
    let error = crate::Error::from_api(api)
        .screen_diagnostic_fallback(|text| reflects_credential(text, credentials));
    Err(Box::new(error))
}

pub(crate) fn error_message_matches(body: &[u8], predicate: impl FnOnce(&str) -> bool) -> bool {
    let fields = [crate::wire::strict_json::Field {
        name: "message",
        children: &[],
    }];
    if crate::wire::strict_json::validate(body, &fields, body.len()).is_err() {
        return false;
    }
    let (_, message) = decode(body, true);
    predicate(&Zeroizing::new(message))
}

pub(crate) fn sanitize_error_text(
    text: &str,
    credentials: &SentCredentials,
) -> crate::error::SafeText {
    use crate::error::SafeText;

    let secrets: Vec<_> = credentials.values().iter().collect();
    let sanitized = SafeText::new(text, &secrets);
    if unsafe_message(text, credentials) || unsafe_message(sanitized.as_str(), credentials) {
        SafeText::new("", &[])
    } else {
        sanitized
    }
}

fn contains_address_marker(text: &str) -> bool {
    text.contains(['/', '@', '\\'])
        || text.to_ascii_lowercase().contains("www.")
        || text
            .chars()
            .zip(text.chars().skip(1))
            .any(|(current, next)| current == ':' && !next.is_whitespace())
}

fn unsafe_message(text: &str, credentials: &SentCredentials) -> bool {
    if contains_address_marker(text) {
        return true;
    }
    if !credentials.values().is_empty() && !text.is_ascii() {
        return true;
    }
    reflects_credential_forms(text, credentials)
}

pub(crate) fn reflects_credential_forms(text: &str, credentials: &SentCredentials) -> bool {
    matches_credential_forms(text.as_bytes(), credentials, MatchContext::ErrorText)
}

pub(super) fn reflects_credential(text: &str, credentials: &SentCredentials) -> bool {
    !credentials.values().is_empty()
        && matches_credential_forms(text.as_bytes(), credentials, MatchContext::Text)
}

pub(super) fn reflects_identifier(value: &str, credentials: &SentCredentials) -> bool {
    !credentials.values().is_empty()
        && matches_credential_forms(value.as_bytes(), credentials, MatchContext::Identifier)
}

pub(crate) fn reflects_bytes(bytes: &[u8], credentials: &SentCredentials) -> bool {
    !credentials.values().is_empty()
        && matches_credential_forms(bytes, credentials, MatchContext::Text)
}

#[derive(Clone, Copy)]
enum MatchContext {
    ErrorText,
    Text,
    Identifier,
}

fn matches_credential_forms(
    text: &[u8],
    credentials: &SentCredentials,
    context: MatchContext,
) -> bool {
    let Some(patterns) = credentials.patterns() else {
        return true;
    };
    let error_text = matches!(context, MatchContext::ErrorText);
    let mut state = MatchState {
        work: matching_work_limit(text.len(), error_text),
        forms: Vec::new(),
        visited: Vec::new(),
        encoded_needles: &patterns.base64,
        hex_needles: &patterns.hex,
        error_text,
        strict_base64: matches!(context, MatchContext::Identifier),
    };
    matches_forms(text, &patterns.literal, [0; 4], &mut state)
}

fn matching_work_limit(length: usize, error_text: bool) -> usize {
    const LIMIT: usize = 8 << 20;
    if error_text {
        LIMIT
    } else {
        length.saturating_mul(64).clamp(LIMIT, 24 * LIMIT)
    }
}

struct MatchState<'a> {
    work: usize,
    forms: Vec<Zeroizing<Vec<u8>>>,
    visited: Vec<(usize, [u8; 4])>,
    encoded_needles: &'a [EncodedNeedle],
    hex_needles: &'a [Zeroizing<String>],
    error_text: bool,
    strict_base64: bool,
}

impl MatchState<'_> {
    fn charge(&mut self, work: usize) -> bool {
        if work >= self.work {
            return false;
        }
        self.work -= work;
        true
    }
}

fn matches_forms(
    input: &[u8],
    needles: &[Zeroizing<Vec<u8>>],
    counts: [u8; 4],
    state: &mut MatchState<'_>,
) -> bool {
    let mut found = None;
    for index in 0..state.forms.len() {
        if !state.charge(input.len()) {
            return true;
        }
        if state.forms[index].as_slice() == input {
            found = Some(index);
            break;
        }
    }
    let index = if let Some(index) = found {
        index
    } else {
        if !state.charge(input.len()) {
            return true;
        }
        for index in 0..state.encoded_needles.len() {
            if !state.charge(
                input
                    .len()
                    .saturating_add(state.encoded_needles[index].len()),
            ) {
                return true;
            }
            if state.encoded_needles[index].matches(input, state.strict_base64) {
                return true;
            }
        }
        let lower = Zeroizing::new(input.to_ascii_lowercase());
        for index in 0..state.hex_needles.len() {
            if !state.charge(lower.len().saturating_add(state.hex_needles[index].len()))
                || encoded::contains_hex(&lower, &state.hex_needles[index], state.strict_base64)
            {
                return true;
            }
        }
        if state.error_text && marker::has_credential_marker_bytes(input) {
            return true;
        }
        if !needles.is_empty() {
            if state.error_text && std::str::from_utf8(&lower).is_err() {
                return true;
            }
            for needle in needles {
                if !state.charge(lower.len().saturating_add(needle.len())) {
                    return true;
                }
                let Ok(needle) = std::str::from_utf8(needle) else {
                    return true;
                };
                if lower
                    .utf8_chunks()
                    .any(|chunk| chunk.valid().contains(needle))
                {
                    return true;
                }
            }
        }
        state.forms.push(Zeroizing::new(input.to_vec()));
        state.forms.len() - 1
    };
    if state.visited.contains(&(index, counts)) {
        return false;
    }
    state.visited.push((index, counts));
    for step in 0..4 {
        if !state.charge(input.len()) {
            return true;
        }
        let next = match step {
            0 => Zeroizing::new(
                input
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_control())
                    .collect(),
            ),
            1 => json_unescape(input),
            2 => Zeroizing::new(percent_decode(input)),
            _ if needles
                .iter()
                .map(|needle| needle.len().saturating_mul(8).saturating_add(5) / 6)
                .min()
                .is_some_and(|minimum| input.len() >= minimum) =>
            {
                decode_base64(input).unwrap_or_else(|| Zeroizing::new(input.to_vec()))
            }
            _ => Zeroizing::new(input.to_vec()),
        };
        if next.as_slice() != input {
            if counts[step] == 2 {
                if step == 3 {
                    let lower = Zeroizing::new(next.to_ascii_lowercase());
                    if needles.iter().any(|needle| {
                        lower
                            .windows(needle.len())
                            .any(|window| window == needle.as_slice())
                    }) {
                        return true;
                    }
                    continue;
                }
                return true;
            }
            let mut next_counts = counts;
            next_counts[step] += 1;
            let error_text = state.error_text;
            if step == 3 {
                state.error_text = false;
            }
            let matched = matches_forms(&next, needles, next_counts, state);
            state.error_text = error_text;
            if matched {
                return true;
            }
        }
    }
    false
}

fn decode_base64(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    [
        data_encoding::BASE64,
        data_encoding::BASE64_NOPAD,
        data_encoding::BASE64URL,
        data_encoding::BASE64URL_NOPAD,
    ]
    .iter()
    .find_map(|encoding| {
        let len = encoding.decode_len(input.len()).ok()?;
        let mut decoded = Zeroizing::new(vec![0; len]);
        let written = encoding.decode_mut(input, &mut decoded).ok()?;
        decoded.truncate(written);
        Some(decoded)
    })
}

fn json_unescape(input: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut unescaped = Zeroizing::new(Vec::with_capacity(input.len()));
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'\\'
            && let Some(&next) = input.get(index + 1)
        {
            if next == b'u' && index + 6 <= input.len() {
                let mut end = index + 6;
                if end + 6 <= input.len() && &input[end..end + 2] == b"\\u" {
                    end += 6;
                }
                let mut quoted = Zeroizing::new(Vec::with_capacity(end - index + 2));
                quoted.push(b'"');
                quoted.extend_from_slice(&input[index..end]);
                quoted.push(b'"');
                if let Ok(value) = serde_json::from_slice::<String>(&quoted) {
                    let value = Zeroizing::new(value);
                    unescaped.extend_from_slice(value.as_bytes());
                    index = end;
                    continue;
                }
                if end != index + 6 {
                    quoted.truncate(1);
                    quoted.extend_from_slice(&input[index..index + 6]);
                    quoted.push(b'"');
                    if let Ok(value) = serde_json::from_slice::<String>(&quoted) {
                        let value = Zeroizing::new(value);
                        unescaped.extend_from_slice(value.as_bytes());
                        index += 6;
                        continue;
                    }
                }
            }
            let decoded = match next {
                b'"' => Some(b'"'),
                b'\\' => Some(b'\\'),
                b'/' => Some(b'/'),
                b'b' => Some(8),
                b'f' => Some(12),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                _ => None,
            };
            if let Some(byte) = decoded {
                unescaped.push(byte);
                index += 2;
                continue;
            }
        }
        unescaped.push(input[index]);
        index += 1;
    }
    unescaped
}

fn retryable(status: StatusCode, retry: Retry) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || (matches!(retry, Retry::Repeatable) && matches!(status.as_u16(), 502..=504))
}

fn status_text(status: StatusCode) -> &'static str {
    match status.as_u16() {
        203 => "Non-Authoritative Information",
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        416 => "Requested Range Not Satisfiable",
        422 => "Unprocessable Entity",
        _ => status.canonical_reason().unwrap_or(""),
    }
}

#[derive(Default)]
struct Envelope {
    code: String,
    message: String,
    error: String,
    detail: String,
    title: String,
    errors: Option<Box<RawValue>>,
    invalid: bool,
}
impl Envelope {
    fn text(&self) -> &str {
        [&self.message, &self.error, &self.detail, &self.title]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("", String::as_str)
    }
}
fn parse_envelope<'de, D: Deserializer<'de>>(
    deserializer: D,
    restricted: bool,
) -> Result<Envelope, D::Error> {
    struct EnvelopeVisitor(bool);
    impl<'de> Visitor<'de> for EnvelopeVisitor {
        type Value = Envelope;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an error object")
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Envelope, E> {
            Ok(Envelope::default())
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Envelope, M::Error> {
            let mut result = Envelope::default();
            while let Some(key) = map.next_key::<String>()? {
                if equal_fold(&key, "code") {
                    let raw = map.next_value::<Box<RawValue>>()?;
                    let value = raw.get();
                    result.code = if value.starts_with('"') {
                        serde_json::from_str(value).unwrap_or_default()
                    } else if value.starts_with('-')
                        || value.starts_with(|c: char| c.is_ascii_digit())
                    {
                        value.to_owned()
                    } else {
                        String::new()
                    };
                } else if !self.0 && equal_fold(&key, "errors") {
                    result.errors = Some(map.next_value()?);
                } else if equal_fold(&key, "message")
                    || (!self.0
                        && (equal_fold(&key, "error")
                            || equal_fold(&key, "detail")
                            || equal_fold(&key, "title")))
                {
                    let raw = map.next_value::<Box<RawValue>>()?;
                    if let Ok(Some(value)) = serde_json::from_str::<Option<String>>(raw.get()) {
                        if equal_fold(&key, "message") {
                            result.message = value;
                        } else if equal_fold(&key, "error") {
                            result.error = value;
                        } else if equal_fold(&key, "detail") {
                            result.detail = value;
                        } else {
                            result.title = value;
                        }
                    } else if raw.get() != "null" {
                        result.invalid = true;
                    }
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_any(EnvelopeVisitor(restricted))
}
impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        parse_envelope(d, false)
    }
}
struct RestrictedEnvelope(Envelope);
impl<'de> Deserialize<'de> for RestrictedEnvelope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        parse_envelope(d, true).map(Self)
    }
}
fn parse_raw(raw: &str, restricted: bool) -> Option<Envelope> {
    if restricted {
        serde_json::from_str::<RestrictedEnvelope>(raw)
            .ok()
            .map(|e| e.0)
    } else {
        serde_json::from_str::<Envelope>(raw).ok()
    }
}
fn equal_fold(value: &str, expected: &str) -> bool {
    value
        .chars()
        .map(|c| match c {
            '\u{017f}' => 's',
            '\u{212a}' => 'k',
            _ => c.to_ascii_lowercase(),
        })
        .eq(expected.chars())
}
fn decode(body: &[u8], restricted: bool) -> (String, String) {
    let trimmed = body.iter().copied().find(|b| !b.is_ascii_whitespace());
    let envelope = if trimmed == Some(b'[') {
        serde_json::from_slice::<Vec<Box<RawValue>>>(body)
            .ok()
            .and_then(|items| {
                let mut parsed = Vec::new();
                for item in items {
                    let raw = item.get().trim_start();
                    if raw != "null" && !raw.starts_with('{') {
                        return None;
                    }
                    let entry = parse_raw(raw, restricted)?;
                    if entry.invalid {
                        return None;
                    }
                    parsed.push(entry);
                }
                parsed.into_iter().next()
            })
            .unwrap_or_default()
    } else {
        std::str::from_utf8(body)
            .ok()
            .and_then(|raw| parse_raw(raw, restricted))
            .unwrap_or_default()
    };
    if !restricted
        && envelope.code.is_empty()
        && envelope.text().is_empty()
        && let Some(raw) = envelope.errors.as_ref()
        && let Ok(entries) = serde_json::from_str::<Vec<Box<RawValue>>>(raw.get())
    {
        let mut first = None;
        let mut messages = Vec::new();
        for entry in entries {
            if !entry.get().trim_start().starts_with('{') {
                continue;
            }
            let item: Envelope = serde_json::from_str(entry.get()).unwrap_or_default();
            if first.is_none() {
                first = Some(item.code.clone());
            }
            if !item.text().is_empty() {
                messages.push(item.text().to_owned());
            }
        }
        return (first.unwrap_or_default(), messages.join("; "));
    }
    let message = if restricted {
        envelope.message.clone()
    } else {
        envelope.text().to_owned()
    };
    (envelope.code, message)
}

fn percent_decode(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == b'%'
            && index + 2 < data.len()
            && let (Some(a), Some(b)) = (
                (data[index + 1] as char).to_digit(16),
                (data[index + 2] as char).to_digit(16),
            )
        {
            output.push(((a << 4) | b) as u8);
            index += 3;
        } else {
            output.push(data[index]);
            index += 1;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(
        status: u16,
        body: &'static [u8],
        fallback: bool,
        sensitive: bool,
        credentials: &SentCredentials,
    ) -> crate::Error {
        *classify(
            "test",
            StatusCode::from_u16(status).unwrap(),
            Bytes::from_static(body),
            &[StatusCode::OK],
            (
                fallback,
                sensitive,
                super::super::redirect::Retry::Repeatable,
            ),
            credentials,
        )
        .unwrap_err()
    }

    macro_rules! withheld_peer_text {
        ($name:ident, $body:expr) => {
            #[test]
            fn $name() {
                let failure = error(403, $body, false, false, &SentCredentials::default());
                let detail = failure.api_error().unwrap();
                assert!(detail.code() == "Forbidden");
                assert!(detail.message() == "Forbidden");
            }
        };
    }
    withheld_peer_text!(
        withholds_mailto,
        br#"{"code":"mailto:ops@example.test","message":"mailto:ops@example.test"}"#
    );
    withheld_peer_text!(withholds_urn, br#"{"code":"urn:x","message":"urn:x"}"#);
    withheld_peer_text!(withholds_email, br#"{"code":"a@b","message":"a@b"}"#);
    withheld_peer_text!(
        withholds_windows_path,
        br#"{"code":"C:\\x","message":"C:\\x"}"#
    );

    #[test]
    fn preserves_colon_followed_by_space() {
        let failure = error(
            403,
            br#"{"code":"Denied","message":"Error: not found"}"#,
            false,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(failure.api_error().unwrap().message(), "Error: not found");
    }

    #[test]
    fn redacts_partial_and_casefolded_urls() {
        for url in ["//example.test/p", "https:/example.test", "HTTPS://X"] {
            assert!(contains_address_marker(url));
        }
        assert!(!contains_address_marker("access denied"));
        for body in [
            br#"{"code":"//example.test/p","message":"//example.test/p"}"#.as_slice(),
            br#"{"code":"https:/example.test","message":"https:/example.test"}"#,
            br#"{"code":"HTTPS://X","message":"HTTPS://X"}"#,
        ] {
            let failure = error(403, body, false, false, &SentCredentials::default());
            let detail = failure.api_error().unwrap();
            assert_eq!(detail.code(), "Forbidden");
            assert_eq!(detail.message(), "Forbidden");
        }
    }

    #[test]
    fn peer_urls_are_withheld() {
        for body in [
            br#"{"code":"https://example.test/private","message":"www.example.test/private"}"#
                .as_slice(),
            br#"{"message":"visit example.test/private"}"#,
        ] {
            let e = error(403, body, true, false, &SentCredentials::default());
            assert_eq!(e.api_error().unwrap().code(), "Forbidden");
            assert_eq!(e.api_error().unwrap().message(), "Forbidden");
        }
        let e = error(
            403,
            br#"{"code":"Denied","message":"access denied"}"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "access denied");
    }

    #[test]
    fn status_classes_are_derived() {
        for (status, class) in [
            (404, crate::ErrorClass::NotFound),
            (403, crate::ErrorClass::PermissionDenied),
            (429, crate::ErrorClass::RateLimited),
            (401, crate::ErrorClass::Auth),
            (500, crate::ErrorClass::Api),
        ] {
            let e = error(status, b"{}", true, false, &SentCredentials::default());
            assert_eq!(e.class(), class);
            assert!(!e.has_identity(class));
        }
    }

    #[test]
    fn fallback_is_optional() {
        for body in [b"{}".as_slice(), b"{\"code\":null}", b"{\"code\":404}"] {
            let e = error(404, body, true, false, &SentCredentials::default());
            assert_eq!(e.api_error().unwrap().code(), "NotFound");
        }
        let e = error(404, b"{}", false, false, &SentCredentials::default());
        assert_eq!(e.api_error().unwrap().code(), "");
    }

    #[test]
    fn peer_fields_are_withheld_for_sensitive_paths() {
        let e = error(
            403,
            br#"{ "code":"custom", "message":"private" }"#,
            true,
            true,
            &SentCredentials::default(),
        );
        let api = e.api_error().unwrap();
        assert_eq!(api.code(), "Forbidden");
        assert_eq!(api.message(), "Forbidden");
    }

    #[test]
    fn reflected_credentials_are_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-secret")]);
        for body in [
            br#"{"message":"toy-secret"}"#.as_slice(),
            br#"{"message":"toy%2dsecret"}"#,
            br#"{"message":"toy%2d\u0000secret"}"#,
            br#"{"code":"toy%2Dsecret"}"#,
        ] {
            let e = error(403, body, true, false, &credentials);
            let api = e.api_error().unwrap();
            assert!(!api.message().contains("toy"));
            assert!(!api.code().contains("toy"));
        }
        let e = error(
            403,
            br#"{"code":"custom","message":"allowed"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "allowed");
    }

    #[test]
    fn safe_text_removes_control_characters() {
        let e = error(
            403,
            br#"{"message":"hello\u0000world"}"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "helloworld");
    }

    #[test]
    fn case_insensitive_fields() {
        let e = error(
            403,
            br#"{"Message":"denied"}"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "denied");
    }

    #[test]
    fn invalid_array_entry_discards_envelope() {
        let e = error(
            403,
            br#"[{"message":"denied"},42]"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn numeric_code_keeps_original_token() {
        let e = error(
            403,
            br#"{"code":1E3}"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().code(), "1E3");
    }

    #[test]
    fn credential_marker_is_withheld() {
        let e = error(
            403,
            br#"{"message":"client_secret: private"}"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert!(!e.api_error().unwrap().message().contains("private"));
    }

    #[test]
    fn json_escaped_credentials_are_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy<secret")]);
        let e = error(
            403,
            br#"{"message":"toy\\u003csecret"}"#,
            true,
            false,
            &credentials,
        );
        assert!(!e.api_error().unwrap().message().contains("toy"));
    }

    #[test]
    fn sanitized_escaped_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy<secret")]);
        let e = error(
            403,
            br#"{"message":"toy\\u00\u00003csecret"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn sanitized_case_insensitive_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-key")]);
        let e = error(
            403,
            br#"{"message":"TOY\u0000-KEY"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn sanitized_unicode_escape_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy\u{2028}secret")]);
        let e = error(
            403,
            br#"{"message":"toy\\u20\u000028secret"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn encoded_uppercase_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("Toy-key")]);
        let e = error(
            403,
            br#"{"message":"%54oy-key"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn mixed_encoded_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("Toy-key")]);
        let e = error(
            403,
            br#"{"message":"%54\\u006fy-key"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn encoded_literal_percent_credential_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("tok%41end")]);
        let e = error(
            403,
            br#"{"message":"tok%2541end"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn credential_trimmed_line_is_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("  toy-key-value  ")]);
        let e = error(
            403,
            br#"{"message":"toy-key-value"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn matching_budget_exhaustion_withholds_message() {
        let credentials = SentCredentials::for_test(
            (0..20)
                .map(|i| crate::Secret::new(format!("credential-{i:02}")))
                .collect(),
        );
        let body = format!("{{\"message\":\"{}\"}}", "ordinary%2520".repeat(40_000));
        let e = classify(
            "test",
            StatusCode::FORBIDDEN,
            Bytes::from(body),
            &[StatusCode::OK],
            (true, false, Retry::Repeatable),
            &credentials,
        )
        .unwrap_err();
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn large_clean_error_message_remains_visible() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-token")]);
        let message = format!("ordinary {}", "z".repeat(650 * 1024));
        assert!(!unsafe_message(&message, &credentials));
        let body = serde_json::json!({"message": message}).to_string();
        let failure = classify(
            "test",
            StatusCode::FORBIDDEN,
            Bytes::from(body),
            &[StatusCode::OK],
            (true, false, Retry::Repeatable),
            &credentials,
        )
        .unwrap_err();
        assert!(failure.api_error().unwrap().message() == &message[..512]);
    }

    #[test]
    fn short_base64_matches_require_token_boundaries() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("1")]);
        let failure = error(
            403,
            br#"{"message":"AMQP connection failed"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(
            failure.api_error().unwrap().message(),
            "AMQP connection failed"
        );
        for encoding in [
            data_encoding::BASE64,
            data_encoding::BASE64_NOPAD,
            data_encoding::BASE64URL,
            data_encoding::BASE64URL_NOPAD,
        ] {
            let text = format!("code={};", encoding.encode(b"1"));
            assert!(reflects_credential(&text, &credentials));
            assert!(sanitize_error_text(&text, &credentials).as_str().is_empty());
        }
    }

    #[test]
    fn short_base64_boundaries_follow_the_matching_variant() {
        let bearer = "ab>de";
        let credentials = SentCredentials::for_test(vec![crate::Secret::new(bearer)]);
        let standard = data_encoding::BASE64_NOPAD.encode(bearer.as_bytes());
        let url_safe = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
        assert!(reflects_credential_forms(
            &format!("group-{standard}-tail"),
            &credentials
        ));
        assert!(reflects_credential_forms(
            &format!("path/{url_safe}/next"),
            &credentials
        ));
        assert!(!reflects_credential_forms(
            &format!("group-{url_safe}-tail"),
            &credentials
        ));
        assert!(!reflects_credential_forms(
            &format!("path/{standard}/next"),
            &credentials
        ));
    }

    #[test]
    fn embedded_complete_short_base64_credentials_are_withheld() {
        for secret in ["a?bcd", "123456"] {
            let credentials = SentCredentials::for_test(vec![crate::Secret::new(secret)]);
            for encoding in [
                data_encoding::BASE64,
                data_encoding::BASE64_NOPAD,
                data_encoding::BASE64URL,
                data_encoding::BASE64URL_NOPAD,
            ] {
                let encoded = encoding.encode(secret.as_bytes());
                let text = format!("prefix {encoded} suffix");
                assert!(reflects_credential(&text, &credentials));
                let body = serde_json::json!({"message": text}).to_string();
                let failure = classify(
                    "test",
                    StatusCode::FORBIDDEN,
                    Bytes::from(body),
                    &[StatusCode::OK],
                    (true, false, Retry::Repeatable),
                    &credentials,
                )
                .unwrap_err();
                assert_eq!(failure.api_error().unwrap().message(), "Forbidden");
            }
            assert!(!reflects_credential("ordinary text", &credentials));
            assert_eq!(
                sanitize_error_text("ordinary text", &credentials).as_str(),
                "ordinary text"
            );
        }
    }

    #[test]
    fn embedded_base64_credentials_are_withheld_at_each_alignment() {
        let secret = "toy-token";
        let credentials = SentCredentials::for_test(vec![crate::Secret::new(secret)]);
        for encoding in [
            data_encoding::BASE64,
            data_encoding::BASE64_NOPAD,
            data_encoding::BASE64URL,
            data_encoding::BASE64URL_NOPAD,
        ] {
            for prefix in ["", "x", "xx"] {
                let encoded = encoding.encode(format!("{prefix}{secret}suffix").as_bytes());
                assert!(reflects_credential(
                    &format!("token={encoded};"),
                    &credentials
                ));
                let body = format!("{{\"message\":\"token={encoded};\"}}");
                let error = classify(
                    "test",
                    StatusCode::FORBIDDEN,
                    Bytes::from(body),
                    &[StatusCode::OK],
                    (true, false, Retry::Repeatable),
                    &credentials,
                )
                .unwrap_err();
                assert_eq!(error.api_error().unwrap().message(), "Forbidden");
            }
        }
    }

    #[test]
    fn short_hex_credential_matches_between_token_boundaries() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("jkl")]);
        assert!(reflects_credential("6A6b6C", &credentials));
        assert!(reflects_credential("id=6A6b6C;", &credentials));
        assert!(reflects_credential("prefix-6A6b6C-suffix", &credentials));
        assert!(!reflects_credential("prefix6A6b6Csuffix", &credentials));
        assert!(!reflects_credential("1236A6b6C9", &credentials));
    }

    #[test]
    fn padded_whole_value_base64_of_a_short_credential_matches_anywhere() {
        for secret in ["abcde", "abcd"] {
            let credentials = SentCredentials::for_test(vec![crate::Secret::new(secret)]);
            let whole = data_encoding::BASE64.encode(secret.as_bytes());
            let embedded = data_encoding::BASE64.encode(format!("xyz{secret}").as_bytes());
            assert!(embedded.contains(&whole));
            assert!(reflects_credential(
                &format!("data {embedded} end"),
                &credentials
            ));
        }
    }

    #[test]
    fn dotted_capital_i_client_secret_marker_is_withheld_from_error_text() {
        let credentials = SentCredentials::default();
        assert!(reflects_credential_forms(
            "CL\u{130}ENT_SECRET: x",
            &credentials
        ));
        assert!(reflects_credential_forms(
            "CLIE\u{b2}NT_SECRET: x",
            &credentials
        ));
        assert!(!reflects_credential_forms("client id", &credentials));
    }

    #[test]
    fn identifier_checks_match_short_hex_anywhere() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("jkl")]);
        assert!(reflects_identifier("prefix6A6b6Csuffix", &credentials));
        assert!(!reflects_credential("prefix6A6b6Csuffix", &credentials));
    }

    #[test]
    fn six_byte_code_is_withheld_at_each_base64_alignment() {
        let secret = "483920";
        let credentials = SentCredentials::for_test(vec![crate::Secret::new(secret)]);
        for encoding in [data_encoding::BASE64_NOPAD, data_encoding::BASE64URL_NOPAD] {
            for prefix in ["", "x", "xx"] {
                let encoded = encoding.encode(format!("{prefix}{secret}").as_bytes());
                assert!(reflects_credential(
                    &format!("token={encoded};"),
                    &credentials
                ));
                let embedded = encoding.encode(format!("{prefix}{secret}suffix").as_bytes());
                assert!(reflects_credential(
                    &format!("token={embedded};"),
                    &credentials
                ));
            }
        }
    }

    #[test]
    fn binary_base64_error_text_remains_visible() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-token")]);
        let e = error(
            401,
            br#"{"message":"Unauthorized"}"#,
            true,
            false,
            &credentials,
        );
        assert_eq!(e.api_error().unwrap().message(), "Unauthorized");
    }

    #[test]
    fn percent_decode_pass_limit_still_fails_closed() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-token")]);
        assert!(reflects_credential("%252541", &credentials));
    }

    #[test]
    fn base64_forms_and_pass_limit_are_withheld() {
        let credentials = SentCredentials::for_test(vec![crate::Secret::new("toy-ÿ-secret")]);
        for encoding in [
            data_encoding::BASE64,
            data_encoding::BASE64_NOPAD,
            data_encoding::BASE64URL,
            data_encoding::BASE64URL_NOPAD,
        ] {
            let encoded = encoding.encode("toy-ÿ-secret".as_bytes());
            assert!(reflects_credential(&encoded, &credentials));
            let nested = encoding.encode(encoding.encode(encoded.as_bytes()).as_bytes());
            assert!(reflects_credential(&nested, &credentials));
            let ordinary = encoding.encode(
                encoding
                    .encode(
                        encoding
                            .encode(b"ordinary text of sufficient length")
                            .as_bytes(),
                    )
                    .as_bytes(),
            );
            assert!(!reflects_credential(&ordinary, &credentials));
        }
        assert!(!reflects_credential("glb-1", &credentials));
    }

    #[test]
    fn output_matching_caps_work_and_fails_closed_on_pathological_input() {
        assert_eq!(matching_work_limit(usize::MAX, false), 192 << 20);
        assert_eq!(matching_work_limit(usize::MAX, true), 8 << 20);
        let credentials = SentCredentials::for_test(
            (0..20)
                .map(|index| crate::Secret::new(format!("sample-{index}")))
                .collect(),
        );
        let text = "ordinary%2520".repeat(1 << 18);
        assert!(reflects_credential(&text, &credentials));
    }

    #[test]
    fn long_prefix_search_is_charged_within_budget() {
        let mut input = vec![b'a'; 1 << 20];
        input[0] = b'!';
        let mut needle = vec![b'a'; 8192];
        needle[8191] = b'b';
        let mut state = MatchState {
            work: 8 << 20,
            forms: Vec::new(),
            visited: Vec::new(),
            encoded_needles: &[],
            hex_needles: &[],
            error_text: true,
            strict_base64: false,
        };
        assert!(!matches_forms(
            &input,
            &[Zeroizing::new(needle)],
            [0; 4],
            &mut state
        ));
        assert_eq!(state.work, (8 << 20) - 6 * (1 << 20) - 8192);
    }

    #[test]
    fn go_status_text_is_used() {
        let e = error(413, b"{}", true, false, &SentCredentials::default());
        assert_eq!(e.api_error().unwrap().message(), "Request Entity Too Large");
    }

    #[test]
    fn retries_follow_operation_policy() {
        for policy in [
            super::super::redirect::Retry::Once,
            super::super::redirect::Retry::NonIdempotent,
        ] {
            assert!(!retryable(StatusCode::SERVICE_UNAVAILABLE, policy));
            assert!(retryable(StatusCode::TOO_MANY_REQUESTS, policy));
        }
        assert!(retryable(
            StatusCode::SERVICE_UNAVAILABLE,
            super::super::redirect::Retry::Repeatable
        ));
    }

    #[test]
    fn object_code_is_not_a_number() {
        let e = error(
            403,
            br#"{"code":{"$serde_json::private::Number":"123"}}"#,
            false,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().code(), "");
    }

    #[test]
    fn large_unknown_field_is_ignored() {
        let body = format!(
            "{{\"unknown\":\"{}\",\"message\":\"denied\"}}",
            "x".repeat(1_000_000)
        );
        let result = classify(
            "test",
            StatusCode::FORBIDDEN,
            Bytes::from(body),
            &[StatusCode::OK],
            (true, false, Retry::Repeatable),
            &SentCredentials::default(),
        )
        .unwrap_err();
        assert_eq!(result.api_error().unwrap().message(), "denied");
    }

    #[test]
    fn invalid_array_field_discards_envelope() {
        let e = error(
            403,
            br#"[{"message":"denied"},{"message":42}]"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn credential_message_non_ascii_is_fixed() {
        let creds = SentCredentials::for_test(vec![crate::Secret::new("toy-key")]);
        let e = error(
            403,
            "{\"message\":\"CLİENT_SECRET\"}".as_bytes(),
            true,
            false,
            &creds,
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn credential_message_ascii_case_insensitive() {
        let creds = SentCredentials::for_test(vec![crate::Secret::new("toy-key")]);
        let e = error(403, br#"{"message":"TOY-KEY"}"#, true, false, &creds);
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn credential_response_only_reads_code_and_message() {
        let creds = SentCredentials::for_test(vec![crate::Secret::new("toy-key")]);
        let e = error(403, br#"{"detail":"peer detail"}"#, true, false, &creds);
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn decimal_array_entry_is_rejected() {
        let e = error(
            403,
            br#"[{"message":"denied"},1.0]"#,
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "Forbidden");
    }

    #[test]
    fn go_203_reason_is_used() {
        let e = error(203, b"{}", true, false, &SentCredentials::default());
        assert_eq!(
            e.api_error().unwrap().message(),
            "Non-Authoritative Information"
        );
    }

    #[test]
    fn unicode_equal_fold_field_names() {
        let e = error(
            403,
            "{\"meſſage\":\"denied\"}".as_bytes(),
            true,
            false,
            &SentCredentials::default(),
        );
        assert_eq!(e.api_error().unwrap().message(), "denied");
    }

    #[test]
    fn success_returns_body() {
        let body = Bytes::from_static(b"result");
        assert_eq!(
            classify(
                "test",
                StatusCode::OK,
                body.clone(),
                &[StatusCode::OK],
                (true, false, super::super::redirect::Retry::Repeatable),
                &SentCredentials::default()
            )
            .unwrap(),
            body
        );
    }
}
