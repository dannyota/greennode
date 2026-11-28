use crate::error::SafeText;
use crate::wire::product::{operation::Operation, response, sender::SentCredentials};
use crate::{ApiError, Error, ErrorClass, LoginError, ServiceAccountLoginError};
use http::StatusCode;
use std::{fmt::Write, ops::Range};
use zeroize::Zeroizing;

pub(super) const HISTORY_WITHHELD: &str = "billing history response withheld";
const AUTH_WITHHELD: &str = "billing history authentication failed";
const REDACTED: &str = "[redacted]";
const MAX_WORK: usize = 8 << 20;
const MAX_FORMS: usize = 256;
const MAX_SPANS: usize = 4096;
const SHORT: usize = 8;

pub(super) fn response_error(
    operation: &Operation,
    status: Option<StatusCode>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation.id,
        status.map(|value| value.as_u16()),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

pub(super) fn invalid_response(operation: &Operation, status: Option<StatusCode>) -> Error {
    response_error(
        operation,
        status,
        "InvalidResponse",
        HISTORY_WITHHELD,
        false,
    )
}

pub(super) fn status_error(
    operation: &Operation,
    status: StatusCode,
    message: &'static str,
) -> Error {
    let classified = response::classify(
        operation.id,
        status,
        bytes::Bytes::new(),
        &[],
        (true, true, operation.retry),
        &SentCredentials::default(),
    )
    .err();
    let api = classified.as_deref().and_then(Error::api_error);
    let code = if status.is_success() || status.is_redirection() && message == HISTORY_WITHHELD {
        "InvalidResponse"
    } else {
        api.map(ApiError::code)
            .filter(|code| !code.is_empty())
            .unwrap_or("RequestFailed")
    };
    response_error(
        operation,
        Some(status),
        code,
        message,
        api.is_some_and(ApiError::retryable),
    )
}

pub(super) fn safe_request_error(
    operation: &Operation,
    error: Error,
    message: &'static str,
) -> Error {
    let api = error.api_error();
    if api.is_some_and(|api| api.message() == message) {
        return error;
    }
    if let Some(login) = error.login_error() {
        return Error::new(error.class()).with_login(LoginError::new(
            AUTH_WITHHELD,
            login.status(),
            login.captcha_suspected(),
        ));
    }
    if let Some(login) = error.service_account_login_error() {
        return Error::new(error.class())
            .with_service_login(ServiceAccountLoginError::new(AUTH_WITHHELD, login.status()));
    }
    if api.is_none() {
        return error;
    }
    let status = api.and_then(|api| StatusCode::from_u16(api.observed_status()?).ok());
    let overflow = error.is_body_too_large();
    let mut safe = match status {
        Some(status) => status_error(operation, status, message),
        None => response_error(
            operation,
            None,
            "RequestFailed",
            message,
            api.is_some_and(ApiError::retryable),
        ),
    }
    .retain_status_visibility(api);
    if overflow {
        safe = safe.with_body_too_large();
    }
    if matches!(
        error.class(),
        ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
    ) {
        return Error::new(error.class()).with_cause(safe);
    }
    safe
}

pub(super) struct Redactor(Option<Vec<Form>>, SentCredentials);

impl Redactor {
    pub(super) fn new(credentials: &SentCredentials) -> Self {
        Self(prepare(credentials), credentials.clone())
    }

    pub(super) fn redact(&self, text: &mut String) {
        if text.is_empty() {
            return;
        }
        if let Some(forms) = &self.0
            && !response::has_credential_marker(text)
            && let Some(spans) = spans(text, forms)
        {
            if spans.is_empty() {
                if self.1.text_reflects(text) {
                    *text = REDACTED.into();
                }
                return;
            }
            let mut output = String::new();
            let mut copied = 0;
            for span in spans {
                output.push_str(&text[copied..span.start]);
                output.push_str(REDACTED);
                copied = span.end;
            }
            output.push_str(&text[copied..]);
            *text = if self.1.text_reflects(&output) {
                REDACTED.into()
            } else {
                output
            };
        } else {
            *text = REDACTED.into();
        }
    }
}

fn prepare(credentials: &SentCredentials) -> Option<Vec<Form>> {
    let mut forms = Vec::<Form>::new();
    let mut budget = MAX_WORK;
    for credential in credentials.values() {
        let value = credential.expose_secret();
        if value.is_empty() {
            continue;
        }
        budget = budget.checked_sub(value.len().checked_mul(12)?)?;
        let escaped = Zeroizing::new(go_escaped(value)?);
        for form in [value, escaped.as_str()] {
            let form = Form::new(form);
            if forms
                .iter()
                .all(|existing| *existing.needle != *form.needle)
            {
                if forms.len() == MAX_FORMS {
                    return None;
                }
                forms.push(form);
            }
        }
    }
    Some(forms)
}

fn go_escaped(value: &str) -> Option<String> {
    let encoded = Zeroizing::new(serde_json::to_string(value).ok()?);
    let mut output = String::with_capacity(encoded.len());
    for ch in encoded.strip_prefix('"')?.strip_suffix('"')?.chars() {
        match ch {
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                write!(output, "\\u{:04x}", u32::from(ch)).ok()?;
            }
            _ => output.push(ch),
        }
    }
    Some(output)
}

fn fold(ch: char) -> char {
    let mut lower = ch.to_lowercase();
    let first = lower.next().unwrap_or(ch);
    if lower.next().is_some() {
        return ch;
    }
    match first {
        'ſ' => 's',
        'ι' | 'ι' => '\u{345}',
        'μ' => 'µ',
        'ς' => 'σ',
        'ϐ' => 'β',
        'ϑ' => 'θ',
        'ϕ' => 'φ',
        'ϖ' => 'π',
        'ϰ' => 'κ',
        'ϱ' => 'ρ',
        'ϵ' => 'ε',
        'ᲀ' => 'в',
        'ᲁ' => 'д',
        'ᲂ' => 'о',
        'ᲃ' => 'с',
        'ᲄ' | 'ᲅ' => 'т',
        'ᲆ' => 'ъ',
        'ᲇ' => 'ѣ',
        'ꙋ' => 'ᲈ',
        'ẛ' => 'ṡ',
        'ΐ' => 'ΐ',
        'ΰ' => 'ΰ',
        'ﬆ' => 'ﬅ',
        _ => first,
    }
}

struct Form {
    needle: Zeroizing<Vec<char>>,
    prefix: Zeroizing<Vec<usize>>,
}

impl Form {
    fn new(text: &str) -> Self {
        let needle = Zeroizing::new(text.chars().map(fold).collect::<Vec<_>>());
        let mut prefix = Zeroizing::new(vec![0; needle.len()]);
        let mut matched = 0;
        for (index, &ch) in needle.iter().enumerate().skip(1) {
            while matched > 0 && needle[matched] != ch {
                matched = prefix[matched - 1];
            }
            if needle[matched] == ch {
                matched += 1;
            }
            prefix[index] = matched;
        }
        Self { needle, prefix }
    }

    fn matches(
        &self,
        value: &str,
        budget: &mut usize,
        spans: &mut Vec<Range<usize>>,
    ) -> Option<()> {
        let size = self.needle.len();
        if value.len() < size {
            return Some(());
        }
        *budget = budget.checked_sub(size)?;
        let mut positions = vec![0; size];
        let mut matched = 0;
        let base = spans.len();
        for (index, (offset, ch)) in value.char_indices().enumerate() {
            *budget = budget.checked_sub(2)?;
            positions[index % size] = offset;
            let folded = fold(ch);
            while matched > 0 && self.needle[matched] != folded {
                *budget = budget.checked_sub(1)?;
                matched = self.prefix[matched - 1];
            }
            if self.needle[matched] == folded {
                matched += 1;
            }
            if matched == size {
                let start = positions[(index + 1 - size) % size];
                let end = offset + ch.len_utf8();
                *budget = budget.checked_sub(end - start)?;
                if end - start < SHORT {
                    return None;
                }
                if spans.len() > base
                    && let Some(last) = spans.last_mut().filter(|last| start < last.end)
                {
                    last.end = end;
                } else {
                    if spans.len() == MAX_SPANS {
                        return None;
                    }
                    spans.push(start..end);
                }
                matched = self.prefix[matched - 1];
            }
        }
        Some(())
    }
}

fn spans(value: &str, forms: &[Form]) -> Option<Vec<Range<usize>>> {
    let mut found = Vec::new();
    let mut budget = MAX_WORK;
    for form in forms {
        form.matches(value, &mut budget, &mut found)?;
    }
    found.sort_unstable_by_key(|span| span.start);
    let mut merged = Vec::<Range<usize>>::new();
    for span in found {
        if let Some(last) = merged.last_mut().filter(|last| span.start < last.end) {
            last.end = last.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    Some(merged)
}

#[cfg(test)]
mod encoded_redaction_tests {
    use super::*;
    use crate::Secret;

    #[test]
    fn history_withholds_encoded_credentials_outside_literal_spans() {
        let credentials = SentCredentials::for_test(vec![Secret::new("toy-history")]);
        let redactor = Redactor::new(&credentials);
        let mut text = String::from("prefix toy%2Dhistory suffix");
        redactor.redact(&mut text);
        assert!(text == REDACTED);
        let mut clean = String::from("ordinary payment");
        redactor.redact(&mut clean);
        assert_eq!(clean, "ordinary payment");
    }
}
