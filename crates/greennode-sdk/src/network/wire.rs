use std::collections::BTreeMap;

use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::endpoint::Base;
use crate::error::SafeText;
use crate::model::wire::{Field, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response::reflects_credential_forms,
    retry,
    sender::SentCredentials,
};
use crate::wire::strict_json::{self, field_matches};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext};

use super::Client;

const CAP: usize = 4 << 20;
const NAT_MESSAGE: &str = "NAT response withheld";
const BANDWIDTH_MESSAGE: &str = "bandwidth response withheld";

pub(super) const PAGING: &[&str] = &[
    "page",
    "pageSize",
    "paging",
    "pagination",
    "totalPage",
    "totalPages",
    "next",
    "nextPage",
    "isNext",
    "hasMore",
    "continuationToken",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    Nat,
    NatCatalog,
    NatCatalogProject,
    NatCatalogRegion,
    NatList,
    EndpointQuote,
    Bandwidth,
    BandwidthProject,
}

impl Style {
    fn is_nat(self) -> bool {
        matches!(
            self,
            Self::Nat
                | Self::NatCatalog
                | Self::NatCatalogProject
                | Self::NatCatalogRegion
                | Self::NatList
        )
    }
}

#[derive(Clone, Copy)]
pub(super) struct Read {
    pub(super) id: &'static str,
    pub(super) template: &'static str,
    pub(super) style: Style,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Read {
    pub(super) const fn new(id: &'static str, template: &'static str, style: Style) -> Self {
        Self {
            id,
            template,
            style,
        }
    }

    pub(super) const fn at(self, template: &'static str, style: Style) -> Self {
        Self {
            template,
            style,
            ..self
        }
    }

    pub(super) fn path(self, field: &str, id: &str) -> Result<(), Error> {
        if PathIds::Strict.allows(id) {
            return Ok(());
        }
        let message = format!("{} requires {field} to match ^[A-Za-z0-9-]+$", self.id);
        Err(
            Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(
                ApiError::without_fallback(
                    "",
                    None,
                    SafeText::new("InvalidUsage", &[]),
                    SafeText::new(&format!("vngcloud: invalid input: {message}"), &[]),
                    false,
                ),
            )),
        )
    }

    pub(super) fn invalid(self, evidence: &Evidence) -> Error {
        self.invalid_as(
            evidence,
            match self.style {
                Style::Nat
                | Style::NatCatalog
                | Style::NatCatalogProject
                | Style::NatCatalogRegion
                | Style::NatList => NAT_MESSAGE,
                Style::EndpointQuote => "endpoint response had invalid structure",
                Style::Bandwidth | Style::BandwidthProject => {
                    "invalid bandwidth response; body withheld"
                }
            },
        )
    }

    pub(super) fn invalid_as(self, evidence: &Evidence, message: &str) -> Error {
        evidence.screen_error(api(
            self.id,
            Some(StatusCode::OK),
            "InvalidResponse",
            message,
            false,
        ))
    }

    pub(super) fn nat_safe_error(self, error: Error, evidence: &Evidence) -> Error {
        let class = error.class();
        let sentinel = match class {
            ErrorClass::Cancelled => Some("context canceled"),
            ErrorClass::DeadlineExceeded => Some("context deadline exceeded"),
            ErrorClass::TokenCache => Some("vngcloud: token cache unavailable"),
            ErrorClass::NoCredentials => {
                Some("vngcloud: invalid config: vngcloud: no credentials found")
            }
            ErrorClass::CredentialsFile => {
                Some("vngcloud: invalid config: vngcloud: credentials file error")
            }
            ErrorClass::InvalidConfig => Some("vngcloud: invalid config"),
            ErrorClass::InvalidInput => Some("vngcloud: invalid input"),
            ErrorClass::MissingProjectId => Some("vngcloud: project id is required"),
            _ => project_selection_message(class),
        };
        if let Some(reason) = sentinel {
            return nat_diagnostic(class, format!("{}: {reason}", self.id), evidence);
        }
        if let Some(details) = error.api_error() {
            let visible_status = details
                .status()
                .and_then(|value| StatusCode::from_u16(value).ok());
            let code = if details.code() == "InvalidResponse"
                && (self.style != Style::Nat || details.observed_status().is_none())
            {
                "InvalidResponse"
            } else {
                ""
            };
            return api(
                self.id,
                visible_status,
                code,
                NAT_MESSAGE,
                details.retryable(),
            )
            .screen_api(&|text| evidence.text_reflects(text))
            .preserve_http_identity(&error);
        }
        if error.has_class(ErrorClass::Auth) {
            return nat_diagnostic(
                ErrorClass::Auth,
                format!("{}: vngcloud: authentication failed", self.id),
                evidence,
            )
            .with_outer_detail(crate::SanitizedAuthentication);
        }
        api(self.id, None, "", "NAT request failed", false)
            .screen_api(&|text| evidence.text_reflects(text))
    }

    fn nat_wire_error(self, error: Error, evidence: &Evidence) -> Error {
        let details = error.api_error();
        let status = details.and_then(|api| StatusCode::from_u16(api.observed_status()?).ok());
        let overflow = error.is_body_too_large();
        let ended = [ErrorClass::Cancelled, ErrorClass::DeadlineExceeded]
            .into_iter()
            .find(|class| error.has_class(*class));
        let invalid = || self.invalid_as(evidence, "NAT response had invalid structure");
        match self.style {
            Style::Nat if let Some(class) = ended => {
                let cause = if class == ErrorClass::Cancelled {
                    "canceled"
                } else {
                    "timed out"
                };
                let details = ApiError::new(
                    self.id,
                    status.map(|status| status.as_u16()),
                    SafeText::new("", &[]),
                    SafeText::new(NAT_MESSAGE, &[]),
                    false,
                )
                .with_request_cause(SafeText::new(cause, &[]));
                Error::new(class).with_cause(Error::from_api(details))
            }
            Style::Nat if overflow => self.withheld(status, false).with_body_too_large(),
            Style::Nat if status == Some(StatusCode::OK) => self.invalid(evidence),
            Style::Nat => self.nat_safe_error(error, evidence),
            Style::NatCatalog if overflow || status.is_some_and(|status| status.is_success()) => {
                invalid()
            }
            Style::NatCatalog => self.nat_safe_error(error, evidence),
            Style::NatCatalogProject if overflow || status == Some(StatusCode::OK) => invalid(),
            Style::NatCatalogRegion if overflow => {
                invalid_nat_regions(evidence, "oversized regions envelope")
            }
            Style::NatCatalogRegion if status == Some(StatusCode::OK) => {
                invalid_nat_regions(evidence, "invalid regions envelope")
            }
            Style::NatCatalogRegion if let Some((status, details)) = status.zip(details) => api(
                self.id,
                Some(status),
                details.code(),
                NAT_MESSAGE,
                details.retryable(),
            )
            .preserve_http_identity(&error),
            Style::NatList if overflow => self.invalid_as(evidence, "oversized list envelope"),
            Style::NatList => {
                let class = error.class();
                error.with_http_identity(class)
            }
            _ => error,
        }
    }

    fn withheld(self, status: Option<StatusCode>, retryable: bool) -> Error {
        let message = match self.style {
            Style::Nat
            | Style::NatCatalog
            | Style::NatCatalogProject
            | Style::NatCatalogRegion
            | Style::NatList => NAT_MESSAGE,
            Style::EndpointQuote => "endpoint response withheld",
            Style::Bandwidth | Style::BandwidthProject => BANDWIDTH_MESSAGE,
        };
        api(self.id, status, "", message, retryable)
    }
}

fn project_selection_message(class: ErrorClass) -> Option<&'static str> {
    match class {
        ErrorClass::ProjectNotFound => Some("vngcloud: no project found for region"),
        ErrorClass::ProjectAmbiguous => Some("vngcloud: multiple projects found for region"),
        _ => None,
    }
}

pub(super) fn shared_nat_project_error(error: Error, region: &str, evidence: &Evidence) -> Error {
    if error.is_body_too_large() {
        return nat_diagnostic(
            ErrorClass::Api,
            "transport: response body exceeds limit".into(),
            evidence,
        )
        .with_body_too_large();
    }
    let class = error.class();
    nat_project_selection_error(error.with_http_identity(class), region, evidence)
}

pub(super) fn nat_project_selection_error(
    error: Error,
    qualifier: &str,
    evidence: &Evidence,
) -> Error {
    match project_selection_message(error.class()) {
        Some(reason) => nat_diagnostic(error.class(), format!("{reason}: {qualifier}"), evidence),
        None => error,
    }
}

pub(super) fn nat_region_selection_error(reason: &str, evidence: &Evidence) -> Error {
    nat_diagnostic(
        ErrorClass::InvalidConfig,
        format!("vngcloud: invalid config: {reason}"),
        evidence,
    )
}

pub(super) fn nat_diagnostic(class: ErrorClass, message: String, evidence: &Evidence) -> Error {
    let error = Error::new(class);
    if evidence.text_reflects(&message) {
        error
    } else {
        error.with_detail(message)
    }
}

pub(super) fn invalid_nat_regions(evidence: &Evidence, reason: &str) -> Error {
    Read::new("network.ListNATInstances", "", Style::NatCatalogRegion).invalid_as(evidence, reason)
}

fn api(
    id: &'static str,
    status: Option<StatusCode>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::new(
        id,
        status.map(|status| status.as_u16()),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

fn guard_bandwidth(
    id: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &SentCredentials,
    sent: &SentCredentials,
) -> Option<Box<Error>> {
    let body = go_json(body);
    (operation::unsafe_json_response(body.as_bytes(), known, sent)
        || (!status.is_success()
            && [known, sent]
                .into_iter()
                .any(|credentials| reflects_credential_forms(&body, credentials))))
    .then(|| {
        let message = if status == StatusCode::OK {
            "invalid bandwidth response; body withheld"
        } else {
            BANDWIDTH_MESSAGE
        };
        Box::new(api(id, Some(status), "InvalidResponse", message, false))
    })
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    pub(super) fn request_evidence(&self, ctx: &RequestContext) -> Result<Evidence, Error> {
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        Ok(evidence)
    }

    pub(super) async fn read(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: Read,
        path: &[&str],
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        self.read_query(
            ctx,
            base,
            read,
            super::inventory::Selection::path(path),
            evidence,
        )
        .await
    }

    pub(super) async fn read_query(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: Read,
        selection: super::inventory::Selection<'_>,
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        self.exchange(
            ctx,
            base,
            read,
            selection,
            (Method::GET, bytes::Bytes::new()),
            evidence,
        )
        .await
    }

    pub(super) async fn exchange(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: Read,
        selection: super::inventory::Selection<'_>,
        (method, body): (Method, bytes::Bytes),
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter).map_err(|error| {
            if read.style.is_nat() {
                read.nat_wire_error(*error, evidence)
            } else {
                *error
            }
        })?;
        let operation = Operation {
            id: read.id,
            template: read.template,
            method,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: CAP,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: matches!(read.style, Style::Nat | Style::EndpointQuote),
            raw_body: false,
            overflow_overrides_status: matches!(
                read.style,
                Style::Nat | Style::Bandwidth | Style::BandwidthProject
            ),
            sensitive: true,
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: read.style != Style::NatList,
            report_redirect: read.style != Style::NatList,
            retry_config: retry::Config::default(),
        };
        #[cfg(test)]
        if read.id == "network.ListVNetworkRegions" {
            operation::assert_nat_region_matches_go(&operation, read.style == Style::Nat);
        }
        let options = Options {
            project_discovery: read.style == Style::BandwidthProject,
            response_guard: match read.style {
                Style::Nat
                | Style::NatCatalog
                | Style::NatCatalogProject
                | Style::NatCatalogRegion
                | Style::NatList
                | Style::EndpointQuote
                | Style::BandwidthProject => None,
                Style::Bandwidth => Some(guard_bandwidth),
            },
            ..Options::DEFAULT
        };
        let screen = |error: Error, evidence: &Evidence| evidence.screen_error(error);
        let response = operation::execute_raw(
            runtime,
            base,
            &operation,
            &options,
            selection.path,
            selection.query,
            RouteAuth::Bearer(self.session.bearer_source()),
            body,
            evidence,
        )
        .await
        .map_err(|error: Failure<()>| {
            let error = error.into_parts().0;
            if read.style.is_nat() {
                return screen(read.nat_wire_error(error, evidence), evidence);
            }
            if read.style == Style::EndpointQuote
                && let Some(interruption) =
                    super::endpoint_quote::endpoint_interruption(&error, evidence)
            {
                return interruption;
            }
            let api = error.api_error();
            let status = api.and_then(|api| StatusCode::from_u16(api.observed_status()?).ok());
            let error = if error.is_body_too_large() {
                if read.style == Style::EndpointQuote {
                    read.invalid(evidence)
                } else {
                    read.withheld(status, false).with_body_too_large()
                }
            } else if matches!(
                error.class(),
                ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
            ) || api.is_some_and(|api| api.code() == "InvalidResponse")
            {
                error
            } else if status.is_some() {
                read.withheld(status, api.is_some_and(ApiError::retryable))
            } else {
                error
            };
            screen(error, evidence)
        })?;
        if response.status != StatusCode::OK {
            let status = response.status;
            if read.style.is_nat() {
                let error = response
                    .classify(read.id, &[], (true, false, Retry::Repeatable), false)
                    .err()
                    .unwrap_or_else(|| Box::new(read.invalid(evidence)));
                return Err(screen(read.nat_wire_error(*error, evidence), evidence));
            }
            if read.style == Style::EndpointQuote && status.is_success() {
                return Err(read.invalid(evidence));
            }
            return Err(screen(
                read.withheld(Some(status), matches!(status.as_u16(), 429 | 502..=504)),
                evidence,
            ));
        }
        if read.style == Style::Nat && !response.empty_body() && response.malformed_json() {
            return Err(read.invalid(evidence));
        }
        response
            .decode_projection(|body| Ok(body.to_vec()))
            .map_err(|error| screen(*error, evidence))
    }

    pub(super) async fn discover_project(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: Read,
        evidence: &mut Evidence,
    ) -> Result<String, Error> {
        let raw = self.read(ctx, base, read, &[], evidence).await?;
        let nat = matches!(read.style, Style::Nat | Style::NatCatalogProject);
        let invalid = || {
            if nat {
                read.invalid_as(evidence, "NAT response had invalid structure")
            } else {
                read.invalid(evidence)
            }
        };
        let text = go_json(&raw);
        let fields = Fields::decode(&text).map_err(|()| invalid())?;
        if nat && !evidence.go_peer_body_screen(&raw).is_clean() {
            return Err(invalid());
        }
        let rows: Vec<Fields<'_>> = fields.required("projects").map_err(|()| invalid())?;
        let incomplete = if nat {
            fields.has_any(PAGING)
        } else {
            fields.bandwidth_incomplete(rows.len())
        };
        if incomplete {
            return Err(invalid());
        }
        let selection_error = |class| {
            let error = Error::new(class);
            if nat {
                nat_project_selection_error(error, read.id, evidence)
            } else {
                error
            }
        };
        let mut selected = None;
        for row in rows {
            let id: String = row
                .optional("projectId")
                .map_err(|()| invalid())?
                .unwrap_or_default();
            let region: String = row
                .optional("region")
                .map_err(|()| invalid())?
                .unwrap_or_default();
            if !region.is_empty() && region != self.region {
                continue;
            }
            if !PathIds::Strict.allows(&id) || evidence.id_reflects(&id) {
                return Err(invalid());
            }
            if selected.replace(id).is_some() {
                return Err(selection_error(ErrorClass::ProjectAmbiguous));
            }
        }
        selected.ok_or_else(|| selection_error(ErrorClass::ProjectNotFound))
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
pub(super) struct Fields<'a>(#[serde(borrow)] BTreeMap<String, &'a RawValue>);

impl<'a> Fields<'a> {
    pub(super) fn decode(text: &'a str) -> Result<Self, ()> {
        Self::decode_with_limit(text, text.len())
    }

    pub(super) fn decode_with_limit(text: &'a str, limit: usize) -> Result<Self, ()> {
        strict_json::validate(text.as_bytes(), &[], limit).map_err(|_| ())?;
        serde_json::from_str(text).map_err(|_| ())
    }

    pub(super) fn raw(&self, name: &str) -> Result<Option<&'a RawValue>, ()> {
        if self
            .0
            .keys()
            .any(|key| key != name && field_matches(key, name))
        {
            return Err(());
        }
        Ok(self.0.get(name).copied())
    }

    pub(super) fn check_ack_paging(&self) -> Result<(), ()> {
        for key in ["page", "size", "total", "totalPage"] {
            self.optional::<crate::model::wire::Field<Option<i64>>>(key)?;
        }
        self.raw("data")?;
        Ok(())
    }

    pub(super) fn output_raw(&self, name: &str) -> Result<Option<&'a RawValue>, ()> {
        let variants = self
            .0
            .keys()
            .filter(|key| key.as_str() != name && field_matches(key, name))
            .count();
        let exact = self.0.get(name).copied();
        if variants > 1 || (variants == 1 && exact.is_some()) {
            return Err(());
        }
        Ok(exact)
    }

    pub(super) fn required<T: Deserialize<'a>>(&self, name: &str) -> Result<T, ()> {
        serde_json::from_str(self.raw(name)?.ok_or(())?.get()).map_err(|_| ())
    }

    pub(super) fn optional<T: Deserialize<'a>>(&self, name: &str) -> Result<Option<T>, ()> {
        self.raw(name)?
            .map(|raw| serde_json::from_str(raw.get()).map_err(|_| ()))
            .transpose()
    }

    pub(super) fn only(&self, names: &[&str]) -> bool {
        self.0.keys().all(|key| names.contains(&key.as_str()))
    }

    pub(super) fn has_any(&self, names: &[&str]) -> bool {
        self.0
            .keys()
            .any(|key| names.iter().any(|name| field_matches(key, name)))
    }

    pub(super) fn bandwidth_incomplete(&self, count: usize) -> bool {
        self.has_any(PAGING)
            || self.has_any(&["nextToken", "cursor", "offset", "limit"])
            || ["total", "totalItem", "totalItems"]
                .into_iter()
                .any(|name| {
                    !self
                        .optional::<Field<Option<i64>>>(name)
                        .is_ok_and(|value| {
                            value.is_none_or(|Field(value)| {
                                value.is_some_and(|value| {
                                    usize::try_from(value).is_ok_and(|value| value <= count)
                                })
                            })
                        })
                })
    }
}
