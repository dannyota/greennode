use super::sender::Response;
use crate::Error;
use http::{Method, StatusCode};
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Clone)]
pub(super) struct RequestIdentity {
    operation: &'static str,
    method: Method,
    route: &'static str,
    requested_target: [u8; 32],
    answered_target: [u8; 32],
}

impl RequestIdentity {
    pub(super) fn operation(&self) -> &'static str {
        self.operation
    }

    pub(super) fn direct(&self) -> Option<CleanRequest> {
        (self.requested_target == self.answered_target).then(|| CleanRequest(self.clone()))
    }

    pub(super) fn new(
        (operation, route): (&'static str, &'static str),
        method: &Method,
        requested: &Url,
        answered: &Url,
    ) -> Self {
        Self {
            operation,
            method: method.clone(),
            route,
            requested_target: Sha256::digest(requested.as_str()).into(),
            answered_target: Sha256::digest(answered.as_str()).into(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct CleanRequest(RequestIdentity);

crate::error::debug_record!(CleanRequest unit);

impl CleanRequest {
    pub(super) fn operation(&self) -> &'static str {
        self.0.operation
    }

    pub(super) fn is_direct(&self) -> bool {
        self.0.requested_target == self.0.answered_target
    }

    pub(crate) fn matches(&self, operation: &str, method: &str, route: &str) -> bool {
        let request = &self.0;
        request.operation == operation
            && request.method.as_str() == method
            && request.route == route
            && self.is_direct()
    }
}

#[derive(Clone)]
pub(crate) struct CleanAbsence(CleanRequest);

crate::error::debug_record!(CleanAbsence unit);

pub(super) fn is_not_found_status(status: StatusCode) -> bool {
    status == StatusCode::NOT_FOUND
}

impl Response {
    pub(crate) fn clean_request(&self) -> Option<CleanRequest> {
        self.reflection.clean()?;
        self.request.as_ref()?.direct()
    }

    pub(crate) fn clean_absence(&self) -> Option<CleanAbsence> {
        is_not_found_status(self.status)
            .then(|| self.clean_request().map(CleanAbsence))
            .flatten()
    }
}

impl Error {
    pub(crate) fn retaining_response_proof(self, classified: &Error) -> Self {
        self.retain_status_visibility(classified.api_error())
            .with_clean_absence(classified.clean_absence_proof())
    }

    pub(crate) fn clean_absence_proof(&self) -> Option<CleanAbsence> {
        self.internal_detail::<CleanAbsence>().cloned()
    }

    pub(crate) fn with_clean_absence(self, proof: Option<CleanAbsence>) -> Self {
        match proof {
            Some(proof) => self.with_outer_detail(proof),
            None => self,
        }
    }
}

/// Reports whether a complete, credential-screened HTTP 404 answered this operation, method and route template.
pub fn confirms_absence(error: &Error, operation: &str, method: &str, route: &str) -> bool {
    error
        .chain()
        .filter_map(|error| error.own_detail::<CleanAbsence>())
        .any(|proof| proof.0.matches(operation, method, route))
}

#[derive(Debug)]
struct UnscreenedNotFoundStatus;

pub(super) fn keep_unscreened_status(error: Error, status: Option<StatusCode>) -> Error {
    if status == Some(StatusCode::NOT_FOUND) {
        error.with_outer_detail(UnscreenedNotFoundStatus)
    } else {
        error
    }
}

pub(crate) fn keep_not_found_diagnostic(
    error: Error,
    original: &Error,
    evidence: &super::evidence::Evidence,
) -> Error {
    let visible = original
        .detail::<crate::HttpStatus>()
        .is_some_and(|status| status.as_u16() == StatusCode::NOT_FOUND.as_u16())
        || original.api_error().and_then(crate::ApiError::status)
            == Some(StatusCode::NOT_FOUND.as_u16());
    if visible && !evidence.text_reflects(StatusCode::NOT_FOUND.as_str()) {
        error.with_http_status(StatusCode::NOT_FOUND)
    } else {
        error.with_observed_http_status(StatusCode::NOT_FOUND)
    }
}

pub(crate) fn is_not_found_diagnostic(error: &Error) -> bool {
    error
        .internal_detail::<UnscreenedNotFoundStatus>()
        .is_some()
        || error.http_status() == Some(StatusCode::NOT_FOUND)
        || error.api_error().and_then(crate::ApiError::observed_status) == Some(404)
}
