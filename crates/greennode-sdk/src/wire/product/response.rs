use super::sender::SentCredentials;
use bytes::Bytes;
use http::StatusCode;
use serde_json::Value;
use zeroize::Zeroizing;

#[allow(dead_code)]
pub(crate) fn classify(
    operation: &'static str,
    status: StatusCode,
    body: Bytes,
    success: &[StatusCode],
    fallback: bool,
    sensitive: bool,
    credentials: &SentCredentials,
) -> Result<Bytes, Box<crate::Error>> {
    if success.contains(&status) {
        return Ok(body);
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let root = parsed
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(&parsed);
    let mut code = field_code(root);
    let mut message = field_message(root).to_owned();
    if code.is_empty()
        && message.is_empty()
        && let Some(entries) = root.get("errors").and_then(Value::as_array)
    {
        let objects: Vec<_> = entries.iter().filter(|v| v.is_object()).collect();
        if let Some(first) = objects.first() {
            code = field_code(first);
        }
        message = objects
            .iter()
            .map(|v| field_message(v))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
    }
    if sensitive {
        code.clear();
        message.clear();
    }
    let secrets: Vec<_> = credentials.values().iter().collect();
    let safe = |text: &str| {
        let sanitized = crate::error::SafeText::new(text, &secrets);
        let decoded = Zeroizing::new(percent_decode(sanitized.as_str().as_bytes()));
        if credentials.reflects(&decoded) {
            crate::error::SafeText::new("", &secrets)
        } else {
            sanitized
        }
    };
    let code = safe(&code);
    let message = if sensitive || message.is_empty() {
        safe(status.canonical_reason().unwrap_or("request failed"))
    } else {
        safe(message.trim())
    };
    let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
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

fn field_code(value: &Value) -> String {
    match value.get("code") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn field_message(value: &Value) -> &str {
    ["message", "error", "detail", "title"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_str))
        .find(|s| !s.is_empty())
        .unwrap_or("")
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
            fallback,
            sensitive,
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
    fn success_returns_body() {
        let body = Bytes::from_static(b"result");
        assert_eq!(
            classify(
                "test",
                StatusCode::OK,
                body.clone(),
                &[StatusCode::OK],
                true,
                false,
                &SentCredentials::default()
            )
            .unwrap(),
            body
        );
    }
}
