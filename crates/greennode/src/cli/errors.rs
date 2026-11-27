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
    pub message: &'a str,
    pub status: Option<u16>,
    pub operation: Option<&'a str>,
}

pub struct Classified<'a> {
    pub envelope: Envelope<'a>,
    pub exit: u8,
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
        .and_then(|api| api.status())
        .filter(|status| *status > 0);
    let has = |class| error.has_identity(class);
    let not_found = has(ErrorClass::NotFound) || status == Some(404);
    let outcome = OUTCOME_NAMES.into_iter().find(|(_, class, _)| has(*class));
    let (rank, code) = if let Some((rank, _, code)) = outcome {
        (rank, code)
    } else if not_found {
        (NOT_FOUND_RANK, "NotFound")
    } else if let Some(api) = api {
        let code = if api.code().is_empty() {
            "RequestFailed"
        } else {
            api.code()
        };
        (51, code)
    } else if has(ErrorClass::NoCredentials) || has(ErrorClass::CredentialsFile) {
        (52, "NoCredentials")
    } else if has(ErrorClass::TokenCache) {
        (53, "TokenCacheFailed")
    } else if error.service_account_login_error().is_some() {
        (54, "ServiceAccountLoginFailed")
    } else if error.login_error().is_some() || has(ErrorClass::Auth) {
        (55, "LoginFailed")
    } else if has(ErrorClass::InvalidInput) {
        (56, "InvalidUsage")
    } else if has(ErrorClass::InvalidConfig) || has(ErrorClass::ProjectAmbiguous) {
        (57, "InvalidConfig")
    } else {
        (PAGE_FORMAT_RANK + 1, "RequestFailed")
    };
    let name = name.filter(|name| name.rank < rank);
    let code = name.map_or(code, |name| name.code);
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
        || status == Some(401)
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
    let message = matching
        .clone()
        .filter_map(|rule| rule.message)
        .next()
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
            api.map(|api| api.message())
                .filter(|message| !message.is_empty())
                .or_else(|| {
                    error
                        .service_account_login_error()
                        .map(|login| login.reason())
                })
                .or_else(|| error.login_error().map(|login| login.reason()))
                .or_else(|| {
                    error
                        .detail::<greennode_sdk::Diagnostic>()
                        .map(|detail| detail.message())
                })
        })
        .unwrap_or("request failed");
    let (status, operation) = match name {
        Some(name) => (
            name.include_api_status.then_some(status).flatten(),
            match name.operation {
                RuleOperation::Omit => None,
                RuleOperation::Fixed(operation) => Some(operation),
                RuleOperation::Api => api.map(|api| api.operation()),
            },
        ),
        None if api.is_some()
            && (code == "NotFound"
                || code == "HistoryIncomplete"
                || code == "RequestFailed"
                || code == api.map_or("", |api| api.code())) =>
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
    write_error_ordered(writer, envelope, false);
}

pub(crate) fn write_error_operation_first(writer: &mut dyn Write, envelope: &Envelope<'_>) {
    write_error_ordered(writer, envelope, true);
}

fn write_error_ordered(writer: &mut dyn Write, envelope: &Envelope<'_>, operation_first: bool) {
    let _ = write!(writer, "{{\"error\":{{\"code\":");
    let _ = write_string(writer, envelope.code);
    let _ = write!(writer, ",\"message\":");
    let _ = write_string(writer, envelope.message);
    if operation_first {
        append_operation(writer, envelope);
    }
    if let Some(status) = envelope.status {
        let _ = write!(writer, ",\"status\":{status}");
    }
    if !operation_first {
        append_operation(writer, envelope);
    }
    let _ = writeln!(writer, "}}}}");
}

fn append_operation(writer: &mut dyn Write, envelope: &Envelope<'_>) {
    if let Some(operation) = envelope.operation {
        let _ = write!(writer, ",\"operation\":");
        let _ = write_string(writer, operation);
    }
}

#[cfg(test)]
mod tests {
    use super::{Envelope, write_error};

    #[test]
    fn error_envelope_uses_go_json_escaping() {
        let envelope = Envelope {
            code: "<code>",
            message: "missing <path> & \"file\"\n\u{2028}\u{2029}\u{007f}",
            status: Some(400),
            operation: Some("a&b"),
        };
        let mut output = Vec::new();
        write_error(&mut output, &envelope);
        assert_eq!(output, "{\"error\":{\"code\":\"\\u003ccode\\u003e\",\"message\":\"missing \\u003cpath\\u003e \\u0026 \\\"file\\\"\\n\\u2028\\u2029\u{007f}\",\"status\":400,\"operation\":\"a\\u0026b\"}}\n".as_bytes());
    }
}
