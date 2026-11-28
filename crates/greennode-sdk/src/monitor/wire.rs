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
    operation::{self, Operation, RouteAuth, execute_raw},
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
    Quota { no_redirect: bool },
}

#[derive(Debug)]
pub(super) struct ResponseReadFailure;

pub(crate) fn response_read_failure(error: Error, safe: Error) -> Error {
    let safe = safe.with_outer_detail(ResponseReadFailure);
    let class = error.class();
    if matches!(class, ErrorClass::Cancelled | ErrorClass::DeadlineExceeded) {
        Error::new(class).with_cause(safe.with_cause(error))
    } else {
        safe
    }
}

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
            Family::CheckAlarm => false,
            Family::Quota { no_redirect } => no_redirect,
        }
    }

    pub(super) fn invalid(&self, evidence: &Evidence) -> Error {
        let message = match self.family {
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
        evidence.screen_error(Error::from_api(ApiError::new(
            self.id,
            status,
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            retryable,
        )))
    }

    fn failure(&self, error: Error, evidence: &Evidence) -> Error {
        let api = error.api_error();
        let status = api.and_then(ApiError::observed_status);
        if error.is_body_too_large() {
            return if matches!(self.family, Family::Dashboard) && status != Some(200) {
                Error::bare_body_limit()
            } else {
                self.overflow(status, evidence)
            };
        }
        if matches!(self.family, Family::Quota { .. } | Family::CheckAlarm)
            && operation::is_active_transport_failure(&error)
        {
            return response_read_failure(error, self.request_failed(status, false, evidence));
        }
        if matches!(self.family, Family::Quota { .. }) && status.is_some() {
            return self.request_failed(status, api.is_some_and(ApiError::retryable), evidence);
        }
        if matches!(self.family, Family::Dashboard) && status.is_none()
            || error.class() != ErrorClass::Api
            || api.is_none()
        {
            return evidence.screen_error(error);
        }
        self.request_failed(status, api.is_some_and(ApiError::retryable), evidence)
    }

    fn overflow(&self, status: Option<u16>, evidence: &Evidence) -> Error {
        let (code, message) = match self.family {
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
    ) -> Result<
        (
            StatusCode,
            Box<RawValue>,
            Option<crate::wire::product::absence::CleanAbsence>,
        ),
        Error,
    > {
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
            keep_status: false,
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: true,
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: read.no_redirect(),
            report_redirect: read.no_redirect(),
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
        let absence = reply.clean_absence();
        if !operation.success.contains(&reply.status) {
            let retryable = matches!(reply.status.as_u16(), 429 | 502 | 503 | 504);
            return Err(read
                .request_failed(Some(reply.status.as_u16()), retryable, evidence)
                .with_clean_absence(absence));
        }
        if reply.status != StatusCode::OK && absence.is_none() {
            return Err(read.error(
                Some(reply.status.as_u16()),
                "InvalidResponse",
                "quota response withheld",
                false,
                evidence,
            ));
        }
        if reply.empty_body() && matches!(read.family, Family::Quota { .. }) {
            let status = reply.status.as_u16();
            let code = if absence.is_some() {
                "NotFound"
            } else {
                "InvalidResponse"
            };
            return Err(read
                .error(
                    Some(status),
                    code,
                    "quota response had invalid structure",
                    false,
                    evidence,
                )
                .with_clean_absence(absence));
        }
        let raw = reply
            .decode_projection(|body| {
                let text = go_json(body);
                serde_json::from_str(&text).map_err(|_| {
                    Box::new(match read.family {
                        Family::Dashboard => read.invalid(evidence),
                        Family::Quota { .. } if absence.is_some() => read
                            .error(
                                Some(reply.status.as_u16()),
                                "NotFound",
                                "quota response withheld",
                                false,
                                evidence,
                            )
                            .with_clean_absence(absence.clone()),
                        Family::CheckAlarm | Family::Quota { .. } => {
                            read.request_failed(Some(reply.status.as_u16()), false, evidence)
                        }
                    })
                })
            })
            .map_err(|error| *error)?;
        Ok((reply.status, raw, absence))
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
    use crate::testing::Server;
    use crate::{Config, ConfigOptions, EndpointOverrides, RequestContext, Secret};

    #[tokio::test]
    async fn dashboard_oversized_redirect_keeps_the_bare_limit_error() {
        let server = Server::start(vec![Server::oversized("302 Found")]);
        let config = Config::new(
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_static_token(Secret::new("toy-dashboard-token"))
                .with_endpoint_overrides(EndpointOverrides {
                    monitor: Some(server.origin.clone()),
                    ..Default::default()
                }),
        )
        .unwrap();
        config
            .session()
            .set_test_product_root(server.root_der.clone());
        let (ctx, _) = RequestContext::new();
        let failure = crate::monitor::Client::new(&config)
            .list_dashboards(&ctx, Default::default())
            .await
            .unwrap_err();
        assert!(failure.error().is_body_too_large());
        assert!(failure.error().api_error().is_none());
        assert_eq!(
            failure.error().to_string(),
            "transport: response body exceeds limit"
        );
    }

    #[tokio::test]
    async fn monitor_active_interruptions_keep_the_service_envelope() {
        use crate::quote::request::{self, Product, Request};
        use crate::testing::{Response, ResponseBodyClock};
        use crate::{ErrorClass, monitor};
        use std::{sync::Arc, time::Duration};

        let mut mismatches = Vec::new();
        for (op, message) in [
            (
                "monitor.GetCheckAlarm",
                "check alarm request failed; response withheld",
            ),
            (
                "monitor.QuoteCreateSyntheticQuota",
                "quota response withheld",
            ),
        ] {
            for end in [ErrorClass::Cancelled, ErrorClass::DeadlineExceeded] {
                for stage in ["headers", "body", "backoff"] {
                    let clock = Arc::new(ResponseBodyClock::new());
                    let (head, request_sent) = tokio::sync::oneshot::channel();
                    let response = match stage {
                        "body" => Response::Stall(head, 10),
                        "backoff" => Response::Http {
                            status: "429 Too Many Requests".into(),
                            headers: vec![
                                ("Retry-After".into(), "1".into()),
                                ("Content-Length".into(), "0".into()),
                            ],
                            body: Vec::new(),
                        },
                        _ => Response::StallBeforeHead(head),
                    };
                    let server = Server::start(vec![response]);
                    let config = Config::new(
                        ConfigOptions::default()
                            .with_region("hcm-3".into())
                            .with_static_token(Secret::new("toy-monitor-token"))
                            .with_clock(clock.clone())
                            .with_endpoint_overrides(EndpointOverrides {
                                monitor: Some(server.origin.clone()),
                                ..Default::default()
                            }),
                    )
                    .unwrap();
                    config
                        .session()
                        .set_test_product_root(server.root_der.clone());
                    let (ctx, cancel) = RequestContext::new();
                    let work = async {
                        if op == "monitor.GetCheckAlarm" {
                            monitor::Client::new(&config)
                                .get_check_alarm(&ctx, monitor::GetCheckAlarmInput::new("check-1"))
                                .await
                                .unwrap_err()
                                .into_parts()
                                .0
                        } else {
                            *request::exchange(
                                config.session(),
                                &ctx,
                                &mut super::Evidence::new(),
                                Request {
                                    operation: op,
                                    template: "billing-api/v2/synthetic/prices/created-price",
                                    product: Product::Monitor {
                                        identifiers: &[],
                                        no_redirect: true,
                                    },
                                    body: Some(&serde_json::json!({})),
                                    path: &[],
                                    query: &[],
                                },
                                None,
                            )
                            .await
                            .unwrap_err()
                        }
                    };
                    let interrupt = async {
                        match stage {
                            "body" => clock.started().await,
                            "backoff" => clock.wait_for_one_second_retry().await,
                            _ => request_sent.await.unwrap(),
                        }
                        if end == ErrorClass::Cancelled {
                            cancel.cancel();
                        } else {
                            clock.advance(Duration::from_secs(120));
                        }
                    };
                    let (error, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                        tokio::join!(work, interrupt)
                    })
                    .await
                    .unwrap();
                    if error.class() != end
                        || !error.has_identity(end)
                        || !error.api_error().is_some_and(|api| {
                            (api.operation(), api.status(), api.code(), api.message())
                                == (op, None, "InvalidResponse", message)
                        })
                    {
                        mismatches.push((op, end, stage));
                    }
                    assert_eq!(server.requests(), 1);
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "unexpected interruption outcomes: {mismatches:?}"
        );
    }

    #[tokio::test]
    async fn quota_cancellation_during_tls_keeps_the_service_envelope() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = Config::new(
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_static_token(Secret::new("toy-monitor-token"))
                .with_endpoint_overrides(EndpointOverrides {
                    monitor: Some(format!("https://{address}")),
                    ..Default::default()
                }),
        )
        .unwrap();
        let (ctx, cancel) = RequestContext::new();
        let client = crate::monitor::Client::new(&config);
        let work = client.list_synthetic_packages(&ctx, crate::monitor::ListSyntheticPackagesInput);
        tokio::pin!(work);
        let handshake = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.read_exact(&mut [0]).await.unwrap();
            stream
        };
        let stream = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::select! { stream = handshake => stream, _ = &mut work => panic!("request ended before TLS cancellation") }
        }).await.unwrap();
        cancel.cancel();
        let failure = work.await.unwrap_err();
        drop(stream);
        let error = failure.error();
        assert!(error.has_identity(crate::ErrorClass::Cancelled));
        let api = error.api_error().unwrap();
        assert_eq!(
            (api.operation(), api.status(), api.code(), api.message()),
            (
                "monitor.ListSyntheticPackages",
                None,
                "InvalidResponse",
                "quota response withheld"
            )
        );
    }
}
