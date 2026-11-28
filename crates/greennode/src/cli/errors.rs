use std::borrow::Cow;
use std::io::Write;

use greennode_sdk::{Error, ErrorClass};

const OUTCOME_NAMES: [(u16, ErrorClass, &str); 5] = [
    (0, ErrorClass::HistoryIncomplete, "HistoryIncomplete"),
    (20, ErrorClass::NotSettled, "NotSettled"),
    (21, ErrorClass::Failed, "WriteFailed"),
    (38, ErrorClass::PriceAboveMax, "PriceAboveMax"),
    (39, ErrorClass::Unpriced, "Unpriced"),
];
const NOT_FOUND_RANK: u16 = 50;
pub const PAGE_FORMAT_RANK: u16 = 58;

#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub identity: &'static str,
    pub matches: fn(&Error) -> bool,
    pub name: Option<RuleName>,
    pub exit: Option<RuleExit>,
    pub message: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct RuleName {
    pub rank: u16,
    pub code: &'static str,
    pub operation: RuleOperation,
    pub include_api_status: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum RuleOperation {
    Omit,
    Fixed(&'static str),
    Api,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleExit {
    Failure,
    Auth,
    NotFound,
    Usage,
}

pub fn validate_rules(rules: &[Rule]) -> bool {
    rules.iter().enumerate().all(|(i, rule)| {
        let rank = rule.name.map(|name| name.rank);
        rank.is_none_or(|rank| {
            rank == PAGE_FORMAT_RANK
                || (rank < NOT_FOUND_RANK
                    && !OUTCOME_NAMES.iter().any(|(used, _, _)| *used == rank))
        }) && rules.iter().skip(i + 1).all(|other| {
            rule.identity != other.identity
                && rank.is_none_or(|rank| other.name.is_none_or(|name| name.rank != rank))
        })
    })
}

pub struct Envelope<'a> {
    pub code: &'a str,
    pub message: Cow<'a, str>,
    pub status: Option<u16>,
    pub operation: Option<&'a str>,
}

pub struct Classified<'a> {
    pub envelope: Envelope<'a>,
    pub exit: u8,
    pub(crate) fixed_message: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameSource {
    Outcome(ErrorClass),
    NotFound,
    Api,
    TokenCache,
    Local,
}

pub fn classify<'a>(error: &'a Error, rules: &'a [Rule]) -> Classified<'a> {
    let api = error.api_error();
    let matching = rules.iter().filter(|rule| (rule.matches)(error));
    let name = matching
        .clone()
        .filter_map(|rule| rule.name)
        .min_by_key(|name| name.rank);
    let matches_exit = |exit| matching.clone().any(|rule| rule.exit == Some(exit));
    let status = api
        .and_then(greennode_sdk::ApiError::response_status)
        .filter(|status| *status > 0);
    let has = |class| error.has_identity(class);
    let not_found = error.has_class(ErrorClass::NotFound);
    let outcome = OUTCOME_NAMES.into_iter().find(|(_, class, _)| has(*class));
    let (rank, code, source) = if let Some((rank, class, code)) = outcome {
        (rank, code, NameSource::Outcome(class))
    } else if not_found {
        (NOT_FOUND_RANK, "NotFound", NameSource::NotFound)
    } else if let Some(api) = api {
        let code = if api.code().is_empty() {
            "RequestFailed"
        } else {
            api.code()
        };
        (51, code, NameSource::Api)
    } else if has(ErrorClass::NoCredentials) || has(ErrorClass::CredentialsFile) {
        (52, "NoCredentials", NameSource::Local)
    } else if has(ErrorClass::TokenCache) {
        (53, "TokenCacheFailed", NameSource::TokenCache)
    } else if error.service_account_login_error().is_some() {
        (54, "ServiceAccountLoginFailed", NameSource::Local)
    } else if error.login_error().is_some()
        || (has(ErrorClass::Auth)
            && error
                .detail::<greennode_sdk::SanitizedAuthentication>()
                .is_none())
    {
        (55, "LoginFailed", NameSource::Local)
    } else if has(ErrorClass::InvalidInput) {
        (56, "InvalidUsage", NameSource::Local)
    } else if has(ErrorClass::InvalidConfig) || has(ErrorClass::ProjectAmbiguous) {
        (57, "InvalidConfig", NameSource::Local)
    } else {
        (PAGE_FORMAT_RANK + 1, "RequestFailed", NameSource::Local)
    };
    let name = name.filter(|name| name.rank < rank);
    let code = name.map_or(code, |name| name.code);
    let token_cache_message = name.is_none() && source == NameSource::TokenCache;
    let recovery_message = error.recovery_diagnostic();
    let fixed_message = token_cache_message || recovery_message.is_some();
    let exit = if matches_exit(RuleExit::Failure)
        || has(ErrorClass::NotSettled)
        || has(ErrorClass::Failed)
        || has(ErrorClass::Cancelled)
        || has(ErrorClass::DeadlineExceeded)
    {
        1
    } else if matches_exit(RuleExit::Auth)
        || has(ErrorClass::NoCredentials)
        || has(ErrorClass::CredentialsFile)
        || has(ErrorClass::Auth)
        || error.login_error().is_some()
        || error.service_account_login_error().is_some()
        || error.is_http_unauthorized()
    {
        3
    } else if matches_exit(RuleExit::NotFound) || not_found {
        4
    } else if matches_exit(RuleExit::Usage)
        || has(ErrorClass::InvalidInput)
        || has(ErrorClass::InvalidConfig)
        || has(ErrorClass::ProjectAmbiguous)
    {
        2
    } else {
        1
    };
    let message = token_cache_message
        .then_some("vngcloud: token cache unavailable")
        .or_else(|| matching.clone().find_map(|rule| rule.message))
        .or_else(|| {
            error
                .detail::<greennode_sdk::tagging::SystemTagText>()
                .map(|text| text.message())
        })
        .or_else(|| {
            error
                .detail::<greennode_sdk::RecoveryAdvice>()
                .map(|advice| advice.message())
        })
        .or_else(|| {
            has(ErrorClass::EstimateLimit)
                .then(|| error.local_diagnostic())
                .flatten()
        })
        .or_else(|| {
            api.map(|api| api.message())
                .filter(|message| !message.is_empty())
        })
        .map(Cow::Borrowed)
        .or_else(|| {
            error
                .service_account_login_error()
                .map(|login| Cow::Owned(login.to_string()))
        })
        .or_else(|| {
            error
                .login_error()
                .map(|login| Cow::Owned(login.to_string()))
        })
        .or_else(|| {
            error
                .detail::<greennode_sdk::Diagnostic>()
                .map(|detail| Cow::Borrowed(detail.message()))
        })
        .unwrap_or_else(|| {
            if let Some(api) = api.filter(|api| {
                api.status().is_none_or(|status| {
                    (200..300).contains(&status) && api.operation().starts_with("billing.")
                })
            }) {
                return Cow::Owned(api.to_string());
            }
            Cow::Borrowed(match error.class() {
                ErrorClass::InvalidInput => "vngcloud: invalid input",
                ErrorClass::Cancelled => "context canceled",
                ErrorClass::DeadlineExceeded => "context deadline exceeded",
                _ => error.diagnostic_fallback(),
            })
        });
    let message = match recovery_message.filter(|message| !message.is_empty()) {
        Some(guidance) if message.is_empty() => Cow::Borrowed(guidance),
        Some(guidance) => Cow::Owned(format!("{message}\n{guidance}")),
        None => message,
    };
    let (status, operation) = match name {
        Some(name) => (
            name.include_api_status.then_some(status).flatten(),
            match name.operation {
                RuleOperation::Omit => None,
                RuleOperation::Fixed(operation) => Some(operation),
                RuleOperation::Api => api.map(|api| api.operation()),
            },
        ),
        None if matches!(
            source,
            NameSource::Api
                | NameSource::NotFound
                | NameSource::Outcome(ErrorClass::HistoryIncomplete)
        ) =>
        {
            (
                status,
                api.map(|api| api.operation()).filter(|op| !op.is_empty()),
            )
        }
        None => (None, None),
    };
    Classified {
        envelope: Envelope {
            code,
            message,
            status,
            operation,
        },
        exit,
        fixed_message,
    }
}

fn write_string(writer: &mut dyn Write, value: &str) -> std::io::Result<()> {
    write!(writer, "\"")?;
    for ch in value.chars() {
        match ch {
            '"' => write!(writer, "\\\"")?,
            '\\' => write!(writer, "\\\\")?,
            '\n' => write!(writer, "\\n")?,
            '\r' => write!(writer, "\\r")?,
            '\t' => write!(writer, "\\t")?,
            '\u{8}' => write!(writer, "\\b")?,
            '\u{c}' => write!(writer, "\\f")?,
            '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                write!(writer, "\\u{:04x}", u32::from(ch))?
            }
            _ => write!(writer, "{ch}")?,
        }
    }
    write!(writer, "\"")
}

pub fn write_error(writer: &mut dyn Write, envelope: &Envelope<'_>) {
    let _ = write!(writer, "{{\"error\":{{\"code\":");
    let _ = write_string(writer, envelope.code);
    let _ = write!(writer, ",\"message\":");
    let _ = write_string(writer, &envelope.message);
    if let Some(status) = envelope.status {
        let _ = write!(writer, ",\"status\":{status}");
    }
    if let Some(operation) = envelope.operation {
        let _ = write!(writer, ",\"operation\":");
        let _ = write_string(writer, operation);
    }
    let _ = writeln!(writer, "}}}}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn peer_code_cannot_add_metadata_to_a_typed_outcome() {
        use greennode_sdk::testing::{api, cause, identity};
        let mut mismatches = Vec::new();
        for (class, code) in [
            (greennode_sdk::ErrorClass::NotSettled, "NotSettled"),
            (greennode_sdk::ErrorClass::Failed, "WriteFailed"),
            (greennode_sdk::ErrorClass::PriceAboveMax, "PriceAboveMax"),
            (greennode_sdk::ErrorClass::Unpriced, "Unpriced"),
        ] {
            let error = cause(
                identity(class),
                api("test.operation", Some(500), code, "safe provider message"),
            );
            let result = super::classify(&error, &[]);
            if (
                result.envelope.code,
                result.envelope.status,
                result.envelope.operation,
            ) != (code, None, None)
            {
                mismatches.push(code);
            }
        }
        assert!(
            mismatches.is_empty(),
            "unexpected HTTP metadata: {mismatches:?}"
        );
    }

    use super::{Envelope, write_error};

    #[test]
    fn error_envelope_uses_go_json_escaping() {
        let envelope = Envelope {
            code: "<code>",
            message: "missing <path> & \"file\"\n\u{2028}\u{2029}\u{007f}".into(),
            status: Some(400),
            operation: Some("a&b"),
        };
        let mut output = Vec::new();
        write_error(&mut output, &envelope);
        assert_eq!(output, "{\"error\":{\"code\":\"\\u003ccode\\u003e\",\"message\":\"missing \\u003cpath\\u003e \\u0026 \\\"file\\\"\\n\\u2028\\u2029\u{007f}\",\"status\":400,\"operation\":\"a\\u0026b\"}}\n".as_bytes());
    }
}
