use std::{collections::BTreeMap, time::Duration};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::value::RawValue;

use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::wire::strict_json::{self, field_matches};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, error::SafeText};

use super::{Client, failure};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadStyle {
    Plain,
    Server,
    Console,
}

pub(super) struct Read {
    pub(super) id: &'static str,
    template: &'static str,
    style: ReadStyle,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Read {
    pub(super) const fn new(id: &'static str, template: &'static str, style: ReadStyle) -> Self {
        Self {
            id,
            template,
            style,
        }
    }

    pub(super) fn invalid(&self, evidence: &Evidence) -> Error {
        match self.style {
            ReadStyle::Server => api(
                self.id,
                Some(200),
                "InvalidResponse",
                "invalid server response; body withheld",
                evidence,
            ),
            ReadStyle::Console => api(
                self.id,
                Some(200),
                "RequestFailed",
                "console log response invalid; body withheld",
                evidence,
            ),
            ReadStyle::Plain => self.decode_error(evidence),
        }
    }

    fn decode_error(&self, evidence: &Evidence) -> Error {
        api(
            self.id,
            None,
            "",
            "response failed to decode; body withheld",
            evidence,
        )
    }

    pub(super) fn path(&self, field: &'static str, id: &str) -> Result<(), Error> {
        if PathIds::Strict.allows(id) {
            return Ok(());
        }
        let details = if id.is_empty() {
            format!("{} requires {field}", self.id)
        } else {
            format!("{} requires {field} to match ^[A-Za-z0-9-]+$", self.id)
        };
        Err(invalid_input(&details))
    }

    pub(super) fn retain<T: WireField, O>(
        &self,
        mut value: T,
        evidence: &Evidence,
    ) -> Result<T, Failure<O>> {
        if value.redact(evidence) {
            return Ok(value);
        }
        Err(failure(api(
            self.id,
            None,
            "InvalidResponse",
            "invalid server response; body withheld",
            evidence,
        )))
    }
}

pub(super) fn invalid_input(detail: &str) -> Error {
    Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(ApiError::without_fallback(
        "",
        None,
        SafeText::new("InvalidUsage", &[]),
        SafeText::new(&format!("vngcloud: invalid input: {detail}"), &[]),
        false,
    )))
}

pub(super) fn api(
    op: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    evidence: &Evidence,
) -> Error {
    Error::from_api(ApiError::without_fallback(
        op,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
    .screen_api(&|text| evidence.text_reflects(text))
}

#[derive(Default)]
pub(super) struct Envelope(BTreeMap<String, Box<RawValue>>);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Envelope {
    pub(super) fn reflects(&self, evidence: &Evidence) -> bool {
        self.0.iter().any(|(key, raw)| {
            evidence.text_reflects(key)
                || serde_json::from_str::<serde_json::Value>(raw.get())
                    .map_or(true, |value| evidence.reflected_in(&value))
        })
    }

    pub(super) fn field<T: WireField, O>(
        &self,
        name: &str,
        read: &Read,
        evidence: &Evidence,
    ) -> Result<T, Failure<O>> {
        let invalid = || failure(read.invalid(evidence));
        if self
            .0
            .keys()
            .any(|key| key != name && field_matches(name, key))
        {
            return Err(invalid());
        }
        let Some(raw) = self.0.get(name) else {
            return Ok(T::default());
        };
        serde_json::from_str::<Field<T>>(raw.get())
            .map(|field| field.0)
            .map_err(|_| invalid())
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    pub(super) fn server_deadline(&self, ctx: &RequestContext) -> RequestContext {
        ctx.child_with_deadline(self.session.clock().monotonic_now() + Duration::from_secs(30))
    }

    pub(super) async fn read<O>(
        &self,
        ctx: &RequestContext,
        read: &Read,
        ids: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<Envelope, Failure<O>> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if ids
            .iter()
            .copied()
            .chain(query.iter().map(|(_, value)| *value))
            .any(|input| evidence.text_reflects(input))
        {
            return Err(failure(Error::new(ErrorClass::InvalidInput)));
        }
        let project = session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| failure(error.screen_api(&|text| evidence.text_reflects(text))))?;
        if std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .any(|input| evidence.text_reflects(input))
        {
            return Err(failure(Error::new(ErrorClass::InvalidInput)));
        }
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let base = session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let protected = read.style != ReadStyle::Plain;
        let operation = Operation {
            id: read.id,
            template: read.template,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: 8 << 20,
            allow_recovery: true,
            allow_empty: true,
            no_output: false,
            keep_status: protected,
            overflow_overrides_status: protected,
            sensitive: protected,
            reject_reflected_credentials: false,
            no_redirect: protected,
            retry_config: retry::Config::default(),
        };
        let result = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            &path,
            query,
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::new(),
            evidence,
        )
        .await;
        let response = result.map_err(|error| {
            let error = error.into_parts().0;
            let status = error
                .api_error()
                .and_then(ApiError::status)
                .or_else(|| error.detail::<StatusCode>().map(|status| status.as_u16()));
            if error.is_body_too_large() {
                return failure(match read.style {
                    ReadStyle::Console => api(
                        read.id,
                        None,
                        "ResponseTooLarge",
                        "console log response exceeds 8 MiB",
                        evidence,
                    ),
                    ReadStyle::Server => api(
                        read.id,
                        status,
                        "InvalidResponse",
                        "invalid server response; body withheld",
                        evidence,
                    ),
                    ReadStyle::Plain => error.screen_api(&|text| evidence.text_reflects(text)),
                });
            }
            if read.style == ReadStyle::Console && status.is_some() {
                return failure(console_status(read, status, evidence));
            }
            failure(error.screen_api(&|text| evidence.text_reflects(text)))
        })?;
        if read.style == ReadStyle::Console && response.status != StatusCode::OK {
            return Err(failure(console_status(
                read,
                Some(response.status.as_u16()),
                evidence,
            )));
        }
        let body = response::classify(
            read.id,
            response.status,
            Bytes::copy_from_slice(&response.body),
            operation.success,
            (true, protected, operation.retry),
            evidence.credentials(),
        )
        .map_err(|error| failure(error.screen_api(&|text| evidence.text_reflects(text))))?;
        if body.is_empty() && !protected {
            return Ok(Envelope::default());
        }
        let text = go_json(&body);
        let syntax_error = || {
            failure(if read.style == ReadStyle::Console {
                read.invalid(evidence)
            } else {
                read.decode_error(evidence)
            })
        };
        let raw = serde_json::from_str::<&RawValue>(&text).map_err(|_| syntax_error())?;
        strict_json::validate(raw.get().as_bytes(), &[], operation.cap)
            .map_err(|_| failure(read.invalid(evidence)))?;
        let fields = serde_json::from_str::<Option<_>>(raw.get())
            .map_err(|_| failure(read.invalid(evidence)))?;
        Ok(Envelope(fields.unwrap_or_default()))
    }
}

fn console_status(read: &Read, status: Option<u16>, evidence: &Evidence) -> Error {
    let code = match status {
        Some(401) => "Unauthorized",
        Some(403) => "Forbidden",
        Some(404) => "NotFound",
        Some(409) => "Conflict",
        Some(429) => "Throttled",
        Some(500..=599) => "ServerError",
        Some(400) => "BadRequest",
        Some(400..=499) => "ClientError",
        _ => "RequestFailed",
    };
    Error::from_api(ApiError::without_fallback(
        read.id,
        status,
        SafeText::new(code, &[]),
        SafeText::new("console log request failed; body withheld", &[]),
        status.is_some_and(|status| matches!(status, 429 | 502..=504)),
    ))
    .screen_api(&|text| evidence.text_reflects(text))
}
