use std::io::Write;

use greennode_sdk::{Error, ErrorClass};

#[derive(Clone, Copy)]
pub struct Rule {
    pub identity: &'static str,
    pub rank: u16,
    pub matches: fn(&Error) -> bool,
    pub code: &'static str,
    pub exit: Option<u8>,
    pub operation: Option<&'static str>,
    pub include_api_status: bool,
}

pub fn validate_rules(rules: &[Rule]) -> bool {
    rules.iter().enumerate().all(|(i, rule)| {
        ![0, 20, 21, 38, 39].contains(&rule.rank)
            && rule.rank < 50
            && rules
                .iter()
                .skip(i + 1)
                .all(|other| rule.identity != other.identity && rule.rank != other.rank)
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
    let rule = matching.clone().min_by_key(|rule| rule.rank);
    let special_exit = matching.filter_map(|rule| rule.exit).min();
    let status = api
        .and_then(|api| api.status())
        .filter(|status| *status > 0);
    let has = |class| error.has_identity(class);
    let not_found = has(ErrorClass::NotFound) || status == Some(404);
    let shared = [
        (0, ErrorClass::HistoryIncomplete, "HistoryIncomplete"),
        (20, ErrorClass::NotSettled, "NotSettled"),
        (21, ErrorClass::Failed, "WriteFailed"),
        (38, ErrorClass::PriceAboveMax, "PriceAboveMax"),
        (39, ErrorClass::Unpriced, "Unpriced"),
    ]
    .into_iter()
    .find(|(_, class, _)| has(*class));
    let rule = rule.filter(|rule| shared.is_none_or(|(rank, _, _)| rule.rank < rank));
    let code = if let Some(rule) = rule {
        rule.code
    } else if let Some((_, _, code)) = shared {
        code
    } else if not_found {
        "NotFound"
    } else if let Some(api) = api {
        if api.code().is_empty() {
            "RequestFailed"
        } else {
            api.code()
        }
    } else if has(ErrorClass::NoCredentials) || has(ErrorClass::CredentialsFile) {
        "NoCredentials"
    } else if has(ErrorClass::TokenCache) {
        "TokenCacheFailed"
    } else if error.service_account_login_error().is_some() {
        "ServiceAccountLoginFailed"
    } else if error.login_error().is_some() || has(ErrorClass::Auth) {
        "LoginFailed"
    } else if has(ErrorClass::InvalidInput) {
        "InvalidUsage"
    } else if has(ErrorClass::InvalidConfig) || has(ErrorClass::ProjectAmbiguous) {
        "InvalidConfig"
    } else {
        "RequestFailed"
    };
    let exit = if special_exit == Some(1)
        || has(ErrorClass::NotSettled)
        || has(ErrorClass::Failed)
        || has(ErrorClass::Cancelled)
        || has(ErrorClass::DeadlineExceeded)
    {
        1
    } else if has(ErrorClass::NoCredentials)
        || has(ErrorClass::CredentialsFile)
        || has(ErrorClass::Auth)
        || error.login_error().is_some()
        || error.service_account_login_error().is_some()
        || status == Some(401)
    {
        3
    } else if not_found {
        4
    } else if has(ErrorClass::InvalidInput)
        || has(ErrorClass::InvalidConfig)
        || has(ErrorClass::ProjectAmbiguous)
    {
        2
    } else {
        special_exit.unwrap_or(1)
    };
    let message = api
        .map(|api| api.message())
        .filter(|message| !message.is_empty())
        .or_else(|| {
            error
                .service_account_login_error()
                .map(|login| login.reason())
        })
        .or_else(|| error.login_error().map(|login| login.reason()))
        .unwrap_or("request failed");
    let (status, operation) = match rule {
        Some(rule) => (
            rule.include_api_status.then_some(status).flatten(),
            rule.operation,
        ),
        None if api.is_some()
            && (code == "NotFound"
                || code == "HistoryIncomplete"
                || code == "RequestFailed"
                || code == api.map_or("", |api| api.code())) =>
        {
            (status, api.map(|api| api.operation()))
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

pub fn write_error(writer: &mut dyn Write, envelope: &Envelope<'_>) {
    let _ = write!(writer, "{{\"error\":{{\"code\":");
    let _ = serde_json::to_writer(&mut *writer, envelope.code);
    let _ = write!(writer, ",\"message\":");
    let _ = serde_json::to_writer(&mut *writer, envelope.message);
    if let Some(status) = envelope.status {
        let _ = write!(writer, ",\"status\":{status}");
    }
    if let Some(operation) = envelope.operation {
        let _ = write!(writer, ",\"operation\":");
        let _ = serde_json::to_writer(&mut *writer, operation);
    }
    let _ = writeln!(writer, "}}}}");
}
