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
    Bandwidth,
    BandwidthProject,
}

#[derive(Clone, Copy)]
pub(super) struct Read {
    id: &'static str,
    template: &'static str,
    style: Style,
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
        let message = if self.style == Style::Nat {
            format!("{} requires a path-safe {field}", self.id)
        } else {
            format!("{} requires {field} to match ^[A-Za-z0-9-]+$", self.id)
        };
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
                Style::Nat => NAT_MESSAGE,
                Style::Bandwidth | Style::BandwidthProject => {
                    "invalid bandwidth response; body withheld"
                }
            },
        )
    }

    pub(super) fn invalid_as(self, evidence: &Evidence, message: &str) -> Error {
        api(
            self.id,
            Some(StatusCode::OK),
            "InvalidResponse",
            message,
            false,
        )
        .screen_api(&|text| evidence.text_reflects(text))
    }

    fn withheld(self, status: Option<StatusCode>, retryable: bool) -> Error {
        let message = match self.style {
            Style::Nat => NAT_MESSAGE,
            Style::Bandwidth | Style::BandwidthProject => BANDWIDTH_MESSAGE,
        };
        api(self.id, status, "", message, retryable)
    }
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
    (operation::unsafe_json_response(body.as_bytes(), known, sent, true)
        || [known, sent]
            .into_iter()
            .any(|credentials| reflects_credential_forms(&body, credentials)))
    .then(|| {
        let message = if status == StatusCode::OK {
            "invalid bandwidth response; body withheld"
        } else {
            BANDWIDTH_MESSAGE
        };
        Box::new(api(id, Some(status), "InvalidResponse", message, false))
    })
}

fn guard_bandwidth_project(
    id: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &SentCredentials,
    sent: &SentCredentials,
) -> Option<Box<Error>> {
    let body = go_json(body);
    (operation::unsafe_json_response(body.as_bytes(), known, sent, status.is_success())
        || [known, sent]
            .into_iter()
            .any(|credentials| reflects_credential_forms(&body, credentials)))
    .then(|| {
        Box::new(api(
            id,
            Some(status),
            "InvalidResponse",
            "invalid discovery response; body withheld",
            false,
        ))
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
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        let operation = Operation {
            id: read.id,
            template: read.template,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: CAP,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: true,
            overflow_overrides_status: true,
            sensitive: true,
            reject_reflected_credentials: false,
            no_redirect: true,
            retry_config: retry::Config::default(),
        };
        let options = Options {
            response_guard: match read.style {
                Style::Nat => None,
                Style::Bandwidth => Some(guard_bandwidth),
                Style::BandwidthProject => Some(guard_bandwidth_project),
            },
            ..Options::DEFAULT
        };
        let screen = |error: Error, evidence: &Evidence| {
            error.screen_api(&|text| evidence.text_reflects(text))
        };
        let response = operation::execute_raw(
            runtime,
            base,
            &operation,
            &options,
            path,
            &[],
            RouteAuth::Bearer(self.session.bearer_source()),
            bytes::Bytes::new(),
            evidence,
        )
        .await
        .map_err(|error: Failure<()>| {
            let error = error.into_parts().0;
            let api = error.api_error();
            let status = error.detail::<StatusCode>().copied().or_else(|| {
                api.and_then(ApiError::status)
                    .and_then(|status| StatusCode::from_u16(status).ok())
            });
            let error = if error.is_body_too_large() {
                read.withheld(status, false).with_body_too_large()
            } else if matches!(
                error.class(),
                ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
            ) {
                if read.style == Style::Nat {
                    error.with_cause(read.withheld(status, false))
                } else {
                    error
                }
            } else if api.is_some_and(|api| api.code() == "InvalidResponse") {
                error
            } else if status.is_some() || (read.style == Style::Nat && api.is_some()) {
                read.withheld(status, api.is_some_and(ApiError::retryable))
            } else {
                error
            };
            screen(error, evidence)
        })?;
        if response.status != StatusCode::OK {
            let status = response.status;
            return Err(screen(
                read.withheld(Some(status), matches!(status.as_u16(), 429 | 502..=504)),
                evidence,
            ));
        }
        if read.style == Style::Nat
            && !response.body.is_empty()
            && serde_json::from_str::<&RawValue>(&go_json(&response.body)).is_err()
        {
            return Err(read.invalid(evidence));
        }
        Ok(response.reply.body)
    }

    pub(super) async fn discover_project(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: Read,
        evidence: &mut Evidence,
    ) -> Result<String, Error> {
        let raw = self.read(ctx, base, read, &[], evidence).await?;
        let invalid = || {
            if read.style == Style::Nat {
                read.invalid_as(evidence, "NAT response had invalid structure")
            } else {
                read.invalid(evidence)
            }
        };
        let text = go_json(&raw);
        let fields = Fields::decode(&text).map_err(|()| invalid())?;
        if read.style == Style::Nat
            && operation::reflects_json(text.as_bytes(), evidence.credentials(), true)
                != Some(false)
        {
            return Err(invalid());
        }
        let rows: Vec<Fields<'_>> = fields.required("projects").map_err(|()| invalid())?;
        let incomplete = if read.style == Style::Nat {
            fields.has_any(PAGING)
        } else {
            fields.bandwidth_incomplete(rows.len())
        };
        if incomplete {
            return Err(invalid());
        }
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
            if !PathIds::Strict.allows(&id) || evidence.text_reflects(&id) {
                return Err(invalid());
            }
            if selected.replace(id).is_some() {
                return Err(Error::new(ErrorClass::ProjectAmbiguous));
            }
        }
        selected.ok_or_else(|| Error::new(ErrorClass::ProjectNotFound))
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
pub(super) struct Fields<'a>(#[serde(borrow)] BTreeMap<String, &'a RawValue>);

impl<'a> Fields<'a> {
    pub(super) fn decode(text: &'a str) -> Result<Self, ()> {
        strict_json::validate(text.as_bytes(), &[], CAP).map_err(|_| ())?;
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

    pub(super) fn required<T: Deserialize<'a>>(&self, name: &str) -> Result<T, ()> {
        serde_json::from_str(self.raw(name)?.ok_or(())?.get()).map_err(|_| ())
    }

    pub(super) fn optional<T: Deserialize<'a>>(&self, name: &str) -> Result<Option<T>, ()> {
        self.raw(name)?
            .map(|raw| serde_json::from_str(raw.get()).map_err(|_| ()))
            .transpose()
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
