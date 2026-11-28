use std::borrow::Cow;

use crate::error::{ApiProvenance, SafeText};
use crate::pricing::EstimateArithmeticError;
use crate::wire::product::evidence::Evidence;
use crate::{ApiError, Error, ErrorClass};

use crate::model::pricing::DiscoveryFailure;

#[derive(Clone, Copy)]
pub(crate) enum CodePolicy {
    Catalog,
    Estimate,
}

pub(crate) fn row(
    operation: &str,
    key: &str,
    error: &Error,
    policy: CodePolicy,
) -> DiscoveryFailure {
    let api = classified_api(error);
    DiscoveryFailure {
        operation: operation.into(),
        item_key: key.into(),
        code: api
            .map_or(Cow::Borrowed("DiscoveryIncomplete"), |api| {
                code(api, policy)
            })
            .into_owned(),
        status: i64::from(api.and_then(ApiError::response_status).unwrap_or_default()),
    }
}

fn classified_api(error: &Error) -> Option<&ApiError> {
    error.api_error().filter(|api| {
        matches!(
            api.provenance(),
            ApiProvenance::Response | ApiProvenance::Transport
        )
    })
}

fn code(api: &ApiError, policy: CodePolicy) -> Cow<'_, str> {
    if api.is_invalid_response() {
        return Cow::Borrowed("InvalidResponse");
    }
    let status = api.observed_response_status();
    if matches!(policy, CodePolicy::Estimate)
        && status == Some(200)
        && let Ok(code) = api.code().parse::<u16>()
        && (code == 112 || (400..=599).contains(&code))
    {
        return Cow::Owned(code.to_string());
    }
    Cow::Borrowed(ApiError::fallback_code(status))
}

pub(crate) fn screen(error: Error, evidence: &Evidence) -> Error {
    let mut error = error;
    if let Some(class) = classified_api(&error).and_then(ApiError::http_class)
        && !error.has_identity(class)
    {
        error = error.with_cause(Error::new(class));
    }
    error.screen_api(&|text| evidence.text_reflects(text))
}

pub(crate) fn estimate_error(error: Error, evidence: &Evidence) -> Error {
    if let Some(arithmetic) = error.detail::<EstimateArithmeticError>()
        && evidence.text_reflects(&arithmetic.item_key)
    {
        let safe = EstimateArithmeticError {
            stage: arithmetic.stage,
            item_key: "<redacted>".into(),
        };
        return error.with_outer_detail(safe);
    }
    if error.has_identity(ErrorClass::EstimateLimit) {
        let error = match error.local_diagnostic() {
            Some(reason) => {
                let text = format!("estimate limit exceeded: {reason}");
                error.with_local_diagnostic(&text)
            }
            None => error,
        };
        return screen(error, evidence);
    }
    let error = if let Some(api) = classified_api(&error) {
        let code = SafeText::new(&code(api, CodePolicy::Estimate), &[]);
        let message = SafeText::new("estimate request failed; body withheld", &[]);
        error.with_api_diagnostic(code, message)
    } else {
        error
    };
    screen(error, evidence)
}

pub(crate) fn copy(error: &Error) -> Error {
    let copy = error.copy_classified();
    if error.has_identity(ErrorClass::EstimateLimit)
        && let Some(reason) = error.local_diagnostic()
    {
        return copy.with_local_diagnostic(reason);
    }
    if let Some(arithmetic) = error.detail::<EstimateArithmeticError>() {
        copy.with_outer_detail(arithmetic.clone())
    } else {
        copy
    }
}

pub(crate) fn screen_rows(rows: &mut [DiscoveryFailure], evidence: &Evidence) {
    for row in rows {
        row.screen_fields(evidence);
    }
}

pub(crate) fn incomplete(errors: impl DoubleEndedIterator<Item = Error>) -> Error {
    let outcome = Error::new(ErrorClass::DiscoveryIncomplete);
    match errors.rev().reduce(|tail, error| error.with_cause(tail)) {
        Some(cause) => outcome.with_cause(cause),
        None => outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_diagnostic_does_not_become_an_api_failure_row() {
        let error =
            crate::quote::request::api("", None, "InvalidUsage", "local input error", false);
        let row = row("pricing.Estimate", "volume", &error, CodePolicy::Estimate);
        assert_eq!((row.code.as_str(), row.status), ("DiscoveryIncomplete", 0));
    }

    #[test]
    fn discovery_validation_classification_survives_code_screening() {
        let mut evidence = Evidence::new();
        evidence.add_secret(&crate::Secret::new("InvalidResponse"));
        let error = crate::quote::request::invalid("portal.ListZones", 403)
            .screen_api(&|text| evidence.text_reflects(text));
        assert_eq!(error.api_error().unwrap().code(), "");
        let mut failure = row("portal.ListZones", "", &error, CodePolicy::Catalog);
        assert_eq!(failure.code, "InvalidResponse");
        failure.screen_fields(&evidence);
        assert!(failure.code.is_empty());
        assert_eq!(failure.status, 403);
    }

    #[test]
    fn aggregation_traverses_copies_screens_and_drops_ten_thousand_failures() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let final_error = Error::new(ErrorClass::PermissionDenied)
                    .with_cause(crate::quote::request::api(
                        "portal.ListZones",
                        Some(403),
                        "",
                        "refused",
                        false,
                    ))
                    .with_outer_detail(10_000usize);
                let error = incomplete(
                    (0..9999)
                        .map(|_| Error::new(ErrorClass::Api))
                        .chain([final_error]),
                );
                assert!(error.has_identity(ErrorClass::PermissionDenied));
                assert_eq!(error.detail::<usize>(), Some(&10_000));
                assert_eq!(
                    error.api_error().and_then(ApiError::response_status),
                    Some(403)
                );
                let copy = copy(&error);
                let mut evidence = Evidence::new();
                evidence.add_secret(&crate::Secret::new("403"));
                let copy = screen(copy, &evidence);
                assert!(copy.has_identity(ErrorClass::PermissionDenied));
                assert_eq!(copy.api_error().and_then(ApiError::response_status), None);
                drop(error);
                drop(copy);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
