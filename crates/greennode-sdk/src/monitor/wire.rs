use bytes::Bytes;
use http::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::Client;
use crate::error::SafeText;
use crate::model::wire::go_json;
use crate::wire::product::options::PathIds;
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
    Plain { strict: bool },
    Dashboard,
    CheckAlarm,
    Quota { no_redirect: bool },
}

pub(super) struct BodyReadFailure;

pub(super) struct Read {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) limit: usize,
    pub(super) family: Family,
}

impl Read {
    fn no_redirect(&self) -> bool {
        match self.family {
            Family::Dashboard => true,
            Family::Plain { .. } | Family::CheckAlarm => false,
            Family::Quota { no_redirect } => no_redirect,
        }
    }

    pub(super) fn invalid(&self, evidence: &Evidence) -> Error {
        let message = match self.family {
            Family::Plain { .. } => "response failed to decode; body withheld",
            Family::Dashboard => "vMonitor response withheld",
            Family::Quota { .. } => "quota response had invalid structure",
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

    fn credential_limit(&self, evidence: &Evidence, status: Option<u16>) -> Option<Error> {
        (matches!(self.family, Family::CheckAlarm) && evidence.credentials().values().len() > 256)
            .then(|| {
                self.error(
                    status,
                    if status.is_some_and(|status| status >= 400) {
                        ""
                    } else {
                        "InvalidResponse"
                    },
                    "check alarm credential safety limit exceeded",
                    false,
                    evidence,
                )
            })
    }

    fn failure(&self, error: Error, evidence: &Evidence) -> Error {
        if let Some(limit) =
            self.credential_limit(evidence, error.api_error().and_then(ApiError::status))
        {
            return limit;
        }
        if matches!(self.family, Family::Plain { .. }) {
            return error.screen_api(&|text| evidence.text_reflects(text));
        }
        let api = error.api_error();
        let status = error
            .detail::<StatusCode>()
            .map(StatusCode::as_u16)
            .or_else(|| api.and_then(ApiError::status))
            .filter(|status| {
                !matches!(self.family, Family::CheckAlarm) || !(300..400).contains(status)
            });
        if error.is_body_too_large() {
            let status = if matches!(self.family, Family::CheckAlarm) {
                None
            } else {
                status
            };
            return self.overflow(status, evidence);
        }
        if matches!(self.family, Family::Quota { .. }) && status.is_some() && api.is_some() {
            let kept_redirect =
                self.no_redirect() && status.is_some_and(|status| (300..400).contains(&status));
            if error.detail::<StatusCode>().is_some() && !kept_redirect {
                return self
                    .request_failed(None, false, evidence)
                    .with_outer_detail(BodyReadFailure);
            }
            return self.request_failed(status, api.is_some_and(ApiError::retryable), evidence);
        }
        if error.class() != ErrorClass::Api || api.is_none() {
            return error.screen_api(&|text| evidence.text_reflects(text));
        }
        self.request_failed(status, api.is_some_and(ApiError::retryable), evidence)
    }

    fn overflow(&self, status: Option<u16>, evidence: &Evidence) -> Error {
        let (code, message) = match self.family {
            Family::Plain { .. } => ("", "transport: response body exceeds limit"),
            Family::Dashboard => ("", "vMonitor response withheld"),
            Family::Quota { .. } => ("ResponseTooLarge", "quota response exceeded size limit"),
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
            Family::Plain { .. } => ("", "response failed to decode; body withheld"),
            Family::Dashboard => ("", "vMonitor response withheld"),
            Family::CheckAlarm | Family::Quota { .. } => (
                if status.is_some_and(|status| status >= 400) {
                    ""
                } else {
                    "InvalidResponse"
                },
                match self.family {
                    Family::Quota { .. } => "quota response withheld",
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
        self.read_status(ctx, read, path, query, evidence, false)
            .await
            .map(|(_, raw, _)| raw)
    }

    pub(super) async fn read_status(
        &self,
        ctx: &RequestContext,
        read: &Read,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
        accept_not_found: bool,
    ) -> Result<(StatusCode, Box<RawValue>, usize), Error> {
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
            success: if accept_not_found {
                &[StatusCode::OK, StatusCode::NOT_FOUND]
            } else {
                &[StatusCode::OK]
            },
            cap: read.limit,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: matches!(read.family, Family::Dashboard | Family::Quota { .. }),
            overflow_overrides_status: false,
            sensitive: !matches!(read.family, Family::Plain { .. }),
            reject_reflected_credentials: false,
            no_redirect: read.no_redirect(),
            retry_config: retry::Config::default(),
        };
        let result = execute_raw(
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
        .await;
        let reply = match result {
            Ok(reply) => reply,
            Err(failure) => return Err(read.failure(failure.into_parts().0, evidence)),
        };
        if let Some(limit) = read.credential_limit(evidence, Some(reply.status.as_u16())) {
            return Err(limit);
        }
        if !operation.success.contains(&reply.status) {
            if matches!(read.family, Family::Plain { .. }) {
                let error = *crate::wire::product::response::classify_full(
                    read.id,
                    reply.status,
                    Bytes::from(reply.reply.body),
                    operation.success,
                    (true, false, operation.retry),
                    evidence.credentials(),
                )
                .err()
                .unwrap_or_else(|| Box::new(read.invalid(evidence)));
                return Err(error.screen_api(&|text| evidence.text_reflects(text)));
            }
            let retryable = matches!(reply.status.as_u16(), 429 | 502 | 503 | 504);
            return Err(read.request_failed(Some(reply.status.as_u16()), retryable, evidence));
        }
        if reply.body.is_empty() && matches!(read.family, Family::CheckAlarm) {
            return Err(read.invalid(evidence));
        }
        if reply.body.is_empty() && matches!(read.family, Family::Quota { .. }) {
            let status = reply.status.as_u16();
            let code = if status == 404 {
                "NotFound"
            } else {
                "InvalidResponse"
            };
            return Err(read.error(
                Some(status),
                code,
                "quota response had invalid structure",
                false,
                evidence,
            ));
        }
        let value_len = reply.body.trim_ascii().len();
        let text =
            if reply.body.is_empty() && matches!(read.family, Family::Plain { strict: false }) {
                std::borrow::Cow::Borrowed("null")
            } else {
                go_json(&reply.body)
            };
        let raw = serde_json::from_str(&text).map_err(|_| match read.family {
            Family::Plain { .. } => read.error(
                None,
                "",
                &format!("{}: request failed: response failed to decode", read.id),
                false,
                evidence,
            ),
            Family::Dashboard => read.invalid(evidence),
            Family::CheckAlarm | Family::Quota { .. } => {
                read.request_failed(Some(reply.status.as_u16()), false, evidence)
            }
        })?;
        Ok((reply.status, raw, value_len))
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

pub(super) fn input_error(text: String) -> Error {
    Error::new(ErrorClass::InvalidInput).with_outer_detail(text)
}

pub(super) fn requires_failure<T>(op: &str, field: &str) -> Failure<T> {
    failure(input_error(format!(
        "vngcloud: invalid input: {op} requires {field}"
    )))
}

pub(super) fn path_id_failure<T>(op: &str, field: &str) -> Failure<T> {
    failure(input_error(format!(
        "vngcloud: invalid input: {op} requires {field} to match ^[A-Za-z0-9-]+$"
    )))
}

pub(super) fn check_path_id<T>(op: &str, field: &str, value: &str) -> Result<(), Failure<T>> {
    if value.is_empty() {
        return Err(requires_failure(op, field));
    }
    if !PathIds::Strict.allows(value) {
        return Err(path_id_failure(op, field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Family, Read};
    use crate::wire::product::evidence::Evidence;
    use crate::{Error, ErrorClass, Secret};

    #[tokio::test]
    async fn check_alarm_failed_request_retains_safe_status_at_credential_limit() {
        use crate::testing::{Product, Response, Server, with_product_loopback};
        use crate::{Config, ConfigOptions, RequestContext};

        let body = vec![b'x'; 64];
        let server = Server::start(vec![Response::Http {
            status: "404 Not Found".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body,
        }]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("ordinary-token"));
        let client = super::Client::new(
            &Config::new(with_product_loopback(Product::Monitor, options, &server)).unwrap(),
        );
        let read = Read {
            id: "monitor.ListCheckAlarmHistory",
            route: "test",
            limit: 16,
            family: Family::CheckAlarm,
        };
        let mut evidence = Evidence::new();
        for i in 0..257 {
            evidence.add_secret(&Secret::new(format!("canary-{i:04}")));
        }
        let (ctx, _) = RequestContext::new();
        let error = client
            .read_status(&ctx, &read, &[], &[], &mut evidence, false)
            .await
            .unwrap_err();
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(404));
        assert_eq!(api.code(), "NotFound");
        assert_eq!(
            api.message(),
            "check alarm credential safety limit exceeded"
        );
    }

    #[tokio::test]
    async fn check_alarm_limit_screens_observed_status_against_credentials() {
        use crate::testing::{Product, Response, Server, with_product_loopback};
        use crate::{Config, ConfigOptions, RequestContext};

        let body = vec![b'x'; 64];
        let server = Server::start(vec![Response::Http {
            status: "404 Not Found".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body,
        }]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("ordinary-token"));
        let client = super::Client::new(
            &Config::new(with_product_loopback(Product::Monitor, options, &server)).unwrap(),
        );
        let read = Read {
            id: "monitor.ListCheckAlarmHistory",
            route: "test",
            limit: 16,
            family: Family::CheckAlarm,
        };
        let mut evidence = Evidence::new();
        for i in 0..257 {
            evidence.add_secret(&Secret::new(format!("canary-{i:04}")));
        }
        evidence.add_secret(&Secret::new("404"));
        let (ctx, _) = RequestContext::new();
        let error = client
            .read_status(&ctx, &read, &[], &[], &mut evidence, false)
            .await
            .unwrap_err();
        assert_eq!(error.api_error().unwrap().status(), None);
    }

    #[test]
    fn check_alarm_failure_refuses_excess_credentials() {
        let read = Read {
            id: "monitor.ListCheckAlarmHistory",
            route: "",
            limit: 1,
            family: Family::CheckAlarm,
        };
        let mut evidence = Evidence::new();
        for i in 0..257 {
            evidence.add_secret(&Secret::new(format!("toy-secret-{i}")));
        }
        let error = read.failure(Error::new(ErrorClass::Api), &evidence);
        assert_eq!(
            error.api_error().unwrap().message(),
            "check alarm credential safety limit exceeded"
        );
    }
}
