use super::*;

const IDENTITIES: [ErrorClass; 10] = [
    ErrorClass::Auth,
    ErrorClass::PermissionDenied,
    ErrorClass::NotFound,
    ErrorClass::RateLimited,
    ErrorClass::Cancelled,
    ErrorClass::DeadlineExceeded,
    ErrorClass::ProjectNotFound,
    ErrorClass::ProjectAmbiguous,
    ErrorClass::InvalidConfig,
    ErrorClass::InvalidInput,
];

#[derive(Debug, PartialEq, Eq)]
struct ApiShape {
    operation: String,
    status: Option<u16>,
    code: String,
    message: String,
    display: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SdkShape {
    api: Option<ApiShape>,
    class: ErrorClass,
    api_class: bool,
    login_details: Option<(String, Option<u16>, bool)>,
    token_cache_identity: bool,
    identities: [bool; IDENTITIES.len()],
    display: String,
}

pub(super) fn actual(error: Error) -> SdkShape {
    let api = error.api_error().map(|api| ApiShape {
        operation: api.operation().to_owned(),
        status: api.status(),
        code: api.code().to_owned(),
        message: api.message().to_owned(),
        display: api.to_string(),
    });
    SdkShape {
        api,
        class: error.class(),
        api_class: error.has_class(ErrorClass::Api),
        login_details: error.login_error().map(|login| {
            (
                login.reason().to_owned(),
                login.status(),
                login.captcha_suspected(),
            )
        }),
        token_cache_identity: error.has_identity(ErrorClass::TokenCache),
        identities: IDENTITIES.map(|class| error.has_identity(class)),
        display: error.to_string(),
    }
}

pub(super) fn expected_sdk(
    want: Option<&Report>,
    target: Target,
    stage: Stage,
    outcome: Outcome,
) -> Option<SdkShape> {
    let (operation, status, code, text) = want?;
    let class = match outcome {
        Outcome::Login => ErrorClass::Auth,
        Outcome::TokenCache => ErrorClass::TokenCache,
        Outcome::WithheldNotFound => ErrorClass::NotFound,
        Outcome::Cancelled | Outcome::CancelledInvalidation => ErrorClass::Cancelled,
        Outcome::Deadline => ErrorClass::DeadlineExceeded,
        Outcome::Empty if stage == Stage::Project => ErrorClass::ProjectNotFound,
        Outcome::Ambiguous if stage == Stage::Project => ErrorClass::ProjectAmbiguous,
        _ => match code.as_str() {
            "InvalidConfig" => ErrorClass::InvalidConfig,
            "InvalidUsage" => ErrorClass::InvalidInput,
            "NotFound" if status.is_none() => ErrorClass::NotFound,
            _ => status.map_or(ErrorClass::Api, |status| http_identity(status).0),
        },
    };
    let api = (!operation.is_empty()).then(|| {
        let is_fallback = text.starts_with(operation) && text.contains(": request failed: ");
        let message = if is_fallback {
            String::new()
        } else {
            text.clone()
        };
        let display = if is_fallback {
            text.clone()
        } else if let Some(status) = status {
            format!("{operation}: {text} (status {status})")
        } else if matches!(target, Target::Detail | Target::Rules)
            && matches!(outcome, Outcome::Cancelled | Outcome::Deadline)
        {
            let cause = match outcome {
                Outcome::Deadline => "timed out",
                _ => "canceled",
            };
            format!("{operation}: {text}: {cause}")
        } else {
            format!("{operation}: {text}")
        };
        ApiShape {
            operation: operation.clone(),
            status: *status,
            code: code.clone(),
            message,
            display,
        }
    });
    let display = api.as_ref().map_or_else(
        || {
            if matches!((target, outcome), (Target::Zones, Outcome::TokenCache)) {
                format!("{}: {text}", target.operation())
            } else {
                text.clone()
            }
        },
        |api| api.display.clone(),
    );
    Some(SdkShape {
        api_class: api.is_some() || class == ErrorClass::Api,
        login_details: matches!((target, outcome), (Target::Instances, Outcome::Login))
            .then(|| ("signin page".to_owned(), None, false)),
        api,
        class,
        token_cache_identity: class == ErrorClass::TokenCache,
        identities: IDENTITIES.map(|identity| identity == class),
        display,
    })
}
