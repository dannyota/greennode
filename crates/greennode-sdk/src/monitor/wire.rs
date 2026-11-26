use bytes::Bytes;
use http::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::Client;
use crate::error::SafeText;
use crate::model::wire::go_json;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, RouteAuth, execute_raw},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::wire::strict_json::{Shape, validate_canonical_shape};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

#[derive(Clone, Copy)]
pub(super) enum Family {
    Dashboard,
    CheckAlarm,
    Quota,
}

pub(super) struct BodyReadFailure;

pub(super) struct Read {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) limit: usize,
    pub(super) family: Family,
}

impl Read {
    pub(super) fn invalid(&self, evidence: &Evidence) -> Error {
        let message = match self.family {
            Family::Dashboard => "vMonitor response withheld",
            Family::Quota => "quota response had invalid structure",
            Family::CheckAlarm => "check alarm response rejected; body withheld",
        };
        self.error(Some(200), "InvalidResponse", message, false, evidence)
    }

    pub(super) fn error(
        &self,
        status: Option<u16>,
        code: &str,
        message: &str,
        retryable: bool,
        evidence: &Evidence,
    ) -> Error {
        Error::from_api(ApiError::new(
            self.id,
            status,
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            retryable,
        ))
        .screen_api(&|text| evidence.text_reflects(text))
    }

    fn failure(&self, error: Error, evidence: &Evidence) -> Error {
        let api = error.api_error();
        let status = error
            .detail::<StatusCode>()
            .map(StatusCode::as_u16)
            .or_else(|| api.and_then(ApiError::status));
        if error.is_body_too_large() {
            return self.overflow(status, evidence);
        }
        if matches!(self.family, Family::Quota) && status.is_some() && api.is_some() {
            let failed =
                self.request_failed(status, api.is_some_and(ApiError::retryable), evidence);
            return if error.detail::<StatusCode>().is_some() {
                failed.with_outer_detail(BodyReadFailure)
            } else {
                failed
            };
        }
        if error.class() != ErrorClass::Api || api.is_none() {
            return error.screen_api(&|text| evidence.text_reflects(text));
        }
        self.request_failed(status, api.is_some_and(ApiError::retryable), evidence)
    }

    fn overflow(&self, status: Option<u16>, evidence: &Evidence) -> Error {
        let (code, message) = match self.family {
            Family::Dashboard => ("", "vMonitor response withheld"),
            Family::Quota => ("ResponseTooLarge", "quota response exceeded size limit"),
            Family::CheckAlarm => (
                "ResponseLimitExceeded",
                "check alarm response exceeds safety limit",
            ),
        };
        self.error(status, code, message, false, evidence)
            .with_body_too_large()
    }

    fn request_failed(&self, status: Option<u16>, retryable: bool, evidence: &Evidence) -> Error {
        let (code, message) = match self.family {
            Family::Dashboard => ("", "vMonitor response withheld"),
            Family::CheckAlarm | Family::Quota => (
                if status.is_some_and(|status| status >= 400) {
                    ""
                } else {
                    "InvalidResponse"
                },
                match self.family {
                    Family::Quota => "quota response withheld",
                    _ => "check alarm request failed; response withheld",
                },
            ),
        };
        self.error(status, code, message, retryable, evidence)
    }
}

impl Client {
    pub(super) async fn read(
        &self,
        ctx: &RequestContext,
        read: &Read,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<Box<RawValue>, Error> {
        let base = self
            .session
            .endpoints()
            .monitor
            .as_ref()
            .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if path.iter().any(|segment| evidence.id_reflects(segment)) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let operation = Operation {
            id: read.id,
            template: read.route,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: read.limit,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: true,
            overflow_overrides_status: false,
            sensitive: true,
            reject_reflected_credentials: false,
            no_redirect: !matches!(read.family, Family::CheckAlarm),
            retry_config: retry::Config::default(),
        };
        let reply = match execute_raw(
            runtime,
            base,
            &operation,
            &Options {
                identifier_query_keys: if read.route == "vmonitor-api/api/v1/statistics/synthetics"
                {
                    &["alarm", "dimensions"]
                } else {
                    &[]
                },
                ..Options::DEFAULT
            },
            path,
            query,
            RouteAuth::Bearer(self.session.bearer_source()),
            Bytes::new(),
            evidence,
        )
        .await
        {
            Ok(reply) => reply,
            Err(failure) => return Err(read.failure(failure.into_parts().0, evidence)),
        };
        if reply.status != StatusCode::OK {
            let retryable = matches!(reply.status.as_u16(), 429 | 502 | 503 | 504);
            return Err(read.request_failed(Some(reply.status.as_u16()), retryable, evidence));
        }
        let text = go_json(&reply.body);
        serde_json::from_str(&text).map_err(|_| match read.family {
            Family::Dashboard => read.invalid(evidence),
            Family::CheckAlarm | Family::Quota => read.request_failed(Some(200), false, evidence),
        })
    }
}

pub(super) fn decode<T: DeserializeOwned>(
    raw: &RawValue,
    shape: &Shape<'_>,
    read: &Read,
    evidence: &Evidence,
) -> Result<T, Error> {
    validate_canonical_shape(raw.get().as_bytes(), shape, raw.get().len())
        .map_err(|_| read.invalid(evidence))?;
    serde_json::from_str(raw.get()).map_err(|_| read.invalid(evidence))
}

pub(super) fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

pub(super) fn invalid_input<T>() -> Failure<T> {
    failure(Error::new(ErrorClass::InvalidInput))
}
