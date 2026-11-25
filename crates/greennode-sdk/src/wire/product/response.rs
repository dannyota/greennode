use super::{redirect::Retry, sender::SentCredentials};
use bytes::Bytes;
use http::StatusCode;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use zeroize::Zeroizing;

#[allow(dead_code)]
pub(crate) fn classify(
    operation: &'static str,
    status: StatusCode,
    body: Bytes,
    success: &[StatusCode],
    rules: (bool, bool, Retry),
    credentials: &SentCredentials,
) -> Result<Bytes, Box<crate::Error>> {
    if success.contains(&status) {
        return Ok(body);
    }
    let (fallback, sensitive, retry) = rules;
    let (code, message) = if sensitive {
        (String::new(), String::new())
    } else {
        decode(&body, !credentials.values().is_empty())
    };
    let secrets: Vec<_> = credentials.values().iter().collect();
    let safe = |text: &str| {
        let sanitized = crate::error::SafeText::new(text, &secrets);
        let decoded = Zeroizing::new(percent_decode(sanitized.as_str().as_bytes()));
        let normalized: String = text
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        let escaped = credentials.values().iter().any(|secret| {
            let json =
                Zeroizing::new(serde_json::to_string(secret.expose_secret()).unwrap_or_default());
            let inner = json.get(1..json.len().saturating_sub(1)).unwrap_or("");
            let html = Zeroizing::new(
                inner
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
                    .replace('&', "\\u0026")
                    .replace('\u{2028}', "\\u2028")
                    .replace('\u{2029}', "\\u2029"),
            );
            (!inner.is_empty() && (text.contains(inner) || text.contains(html.as_str())))
                || secret
                    .expose_secret()
                    .lines()
                    .map(str::trim)
                    .any(|line| line.len() >= 8 && text.contains(line))
        });
        if normalized.contains("clientsecret") || escaped || credentials.reflects(&decoded) {
            crate::error::SafeText::new("", &secrets)
        } else {
            sanitized
        }
    };
    let code = safe(&code);
    let message = if sensitive
        || message.is_empty()
        || (!secrets.is_empty() && unsafe_message(&message, credentials))
    {
        safe(status_text(status))
    } else {
        let value = safe(message.trim());
        if !secrets.is_empty() && value.as_str().is_empty() {
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
    Err(Box::new(crate::Error::from_api(api)))
}

fn unsafe_message(text: &str, credentials: &SentCredentials) -> bool {
    if !text.is_ascii() {
        return true;
    }
    let lower = Zeroizing::new(text.to_ascii_lowercase());
    let decoded = Zeroizing::new(percent_decode(lower.as_bytes()));
    let normalized: String = lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if normalized.contains("clientsecret") {
        return true;
    }
    credentials.values().iter().any(|secret| {
        let value = Zeroizing::new(secret.expose_secret().to_ascii_lowercase());
        if value.is_empty() {
            return false;
        }
        let json = Zeroizing::new(serde_json::to_string(value.as_str()).unwrap_or_default());
        let inner = json.get(1..json.len().saturating_sub(1)).unwrap_or("");
        let html = Zeroizing::new(
            inner
                .replace('<', "\\u003c")
                .replace('>', "\\u003e")
                .replace('&', "\\u0026"),
        );
        lower.contains(value.as_str())
            || (!inner.is_empty() && (lower.contains(inner) || lower.contains(html.as_str())))
            || decoded
                .windows(value.len())
                .any(|part| part == value.as_bytes())
            || value
                .lines()
                .map(str::trim)
                .any(|line| line.len() >= 8 && lower.contains(line))
    })
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
