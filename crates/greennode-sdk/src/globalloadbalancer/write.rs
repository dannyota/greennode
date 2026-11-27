use std::{future::Future, pin::Pin, sync::LazyLock, time::Duration};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use super::{Client, invalid_input, read_operation};
use crate::config::session::AuthKind;
use crate::endpoint::Base;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::Options,
    redirect::{Classification, Retry},
    response,
    sender::{BearerSource, BearerToken, Response, SentCredentials},
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, Secret, WriteDispatch};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WRITE_OK: &[StatusCode] = &[StatusCode::OK, StatusCode::ACCEPTED];
const CREDENTIAL_INPUT: &str = "a request value contains a credential";
pub(super) const PROVENANCE: &str =
    "vngcloud: invalid config: GLB writes require SDK IAM-user login and default endpoints";

pub(super) struct Write<'a> {
    pub(super) client: &'a Client,
    pub(super) id: &'static str,
    pub(super) evidence: Evidence,
}

#[derive(Clone, Copy)]
pub(super) enum Settlement {
    Unknown(&'static str),
    Failed,
}

impl Client {
    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Failure by value"
    )]
    pub(super) fn start_write<O>(&self, id: &'static str) -> Result<Write<'_>, Failure<O>> {
        let current = self.session.endpoints();
        let default = self.session.default_endpoints();
        let same = |current: &Option<Base>, default: &Option<Base>| {
            current.as_ref().map(Base::configured_url) == default.as_ref().map(Base::configured_url)
        };
        let trusted = self.session.auth_kind() == AuthKind::IamUser
            && self.session.uses_default_auth_endpoints()
            && same(&current.global_load_balancer, &default.global_load_balancer)
            && same(&current.dashboard, &default.dashboard);
        #[cfg(all(debug_assertions, any(test, feature = "test-support")))]
        let trusted = trusted
            || self
                .session
                .is_write_fixture(crate::testing::Product::GlobalLoadBalancer);
        if !trusted {
            return Err(not_sent(local(
                ErrorClass::InvalidConfig,
                "InvalidConfig",
                PROVENANCE,
            )));
        }
        Ok(Write {
            client: self,
            id,
            evidence: Evidence::new(),
        })
    }
}

#[allow(clippy::result_large_err)]
impl Write<'_> {
    pub(super) async fn request(
        &mut self,
        ctx: &RequestContext,
        op: &Operation,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<Response, Failure<()>> {
        self.request_on(
            ctx,
            op,
            self.client
                .session
                .endpoints()
                .global_load_balancer
                .as_ref(),
            path,
            body,
        )
        .await
    }

    pub(super) async fn request_on(
        &mut self,
        ctx: &RequestContext,
        op: &Operation,
        base: Option<&Base>,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<Response, Failure<()>> {
        let session = &self.client.session;
        let base = base.ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        if let Some(credentials) = runtime.configured_credentials {
            self.evidence.absorb(credentials);
        }
        if reflected(&self.evidence, path, body) {
            return Err(not_sent(invalid_input(CREDENTIAL_INPUT.into())));
        }
        let guarded = GuardedSource {
            source: session.bearer_source(),
            path,
            body,
        };
        let body = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        operation::execute_raw(
            runtime,
            base,
            op,
            &Options::DEFAULT,
            path,
            &[],
            RouteAuth::Bearer(&guarded),
            body,
            &mut self.evidence,
        )
        .await
    }

    pub(super) fn finish<O: Serialize>(
        &self,
        reason: Settlement,
        partial: Option<O>,
    ) -> Failure<O> {
        let (class, reason) = match reason {
            Settlement::Unknown(reason) => (
                ErrorClass::NotSettled,
                format!("vngcloud: write outcome not confirmed: {reason}"),
            ),
            Settlement::Failed => (ErrorClass::Failed, "vngcloud: write failed".into()),
        };
        let message = format!(
            "{}: inspect inventory and billing before another mutation: {reason}",
            self.id
        );
        let partial =
            partial.filter(|output| self.evidence.can_retain_partial() && self.safe_output(output));
        Failure::new(
            local(class, "", &message),
            partial,
            WriteDispatch::MayHaveBeenSent,
        )
    }

    pub(super) fn safe_output(&self, output: &impl Serialize) -> bool {
        serde_json::to_value(output).is_ok_and(|value| !self.evidence.output_reflects(&value))
    }

    pub(super) fn wait_context(&self, ctx: &RequestContext, duration: Duration) -> RequestContext {
        ctx.child_with_deadline(self.client.session.clock().monotonic_now() + duration)
    }

    pub(super) fn interrupted(&self, ctx: &RequestContext) -> bool {
        ctx.check(self.client.session.clock()).is_some()
    }

    pub(super) async fn pause(&self, ctx: &RequestContext) {
        self.wait_context(ctx, POLL_INTERVAL)
            .wait(self.client.session.clock())
            .await;
    }

    pub(super) fn rejection<O: Serialize>(
        &self,
        failure: Failure<()>,
        partial: Option<O>,
        uncertain: &'static str,
    ) -> Failure<O> {
        self.rejection_with_message(failure, partial, uncertain, None)
    }

    pub(super) fn rejection_with_message<O: Serialize>(
        &self,
        failure: Failure<()>,
        partial: Option<O>,
        uncertain: &'static str,
        fixed_message: Option<&str>,
    ) -> Failure<O> {
        let (error, _, dispatch) = failure.into_parts();
        if dispatch == WriteDispatch::NotSent {
            return Failure::new(error, None, dispatch);
        }
        if let Some(status) = error
            .api_error()
            .and_then(ApiError::status)
            .and_then(|status| StatusCode::from_u16(status).ok())
            .filter(StatusCode::is_client_error)
        {
            let credentials = self.evidence.credentials();
            let error = match fixed_message {
                Some(message) => api_rejection(self.id, status, message, false, credentials),
                None => status_rejection(self.id, status, credentials),
            };
            return Failure::new(error, None, dispatch);
        }
        self.finish(Settlement::Unknown(uncertain), partial)
    }
}

fn reflected(evidence: &Evidence, path: &[&str], body: Option<&Value>) -> bool {
    path.iter().any(|id| evidence.text_reflects(id))
        || body.is_some_and(|body| evidence.output_reflects(body))
}

struct GuardedSource<'a> {
    source: &'a dyn BearerSource,
    path: &'a [&'a str],
    body: Option<&'a Value>,
}

impl BearerSource for GuardedSource<'_> {
    fn refreshable(&self) -> bool {
        self.source.refreshable()
    }

    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, Error>> + Send + 'a>> {
        Box::pin(async move {
            let bearer = self.source.token(ctx).await?;
            let mut evidence = Evidence::new();
            evidence.add_secret(&bearer.token.access_token);
            if let Some(credentials) = &bearer.signin_credentials {
                evidence.absorb(credentials);
            }
            if reflected(&evidence, self.path, self.body) {
                return Err(invalid_input(CREDENTIAL_INPUT.into()));
            }
            Ok(bearer)
        })
    }

    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.source.rejected(ctx, sent, evict)
    }
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

pub(super) fn local(class: ErrorClass, code: &str, message: &str) -> Error {
    Error::new(class).with_cause(api_error("", None, code, message, false))
}

pub(super) fn api_error(
    id: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::without_fallback(
        id,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

pub(super) fn write_operation(
    id: &'static str,
    template: &'static str,
    method: Method,
) -> Operation {
    Operation {
        method,
        class: Classification::Write,
        retry: Retry::Once,
        success: WRITE_OK,
        allow_recovery: false,
        keep_status: true,
        overflow_overrides_status: false,
        ..guard_operation(id, template)
    }
}

pub(super) fn guard_operation(id: &'static str, template: &'static str) -> Operation {
    Operation {
        allow_empty: false,
        overflow_overrides_status: true,
        sensitive: true,
        no_redirect: true,
        ..read_operation(id, template)
    }
}

pub(super) fn status_rejection(
    id: &'static str,
    status: StatusCode,
    credentials: &SentCredentials,
) -> Error {
    api_rejection(id, status, "", false, credentials)
}

pub(super) fn rejected_response(
    op: &Operation,
    reply: &Response,
    body: Option<&Value>,
    fixed_message: Option<&str>,
) -> Error {
    let classified = response::classify(
        op.id,
        reply.status,
        Bytes::copy_from_slice(&reply.body),
        &[],
        (true, fixed_message.is_some(), op.retry),
        &reply.credentials,
    )
    .err();
    let api = classified.as_ref().and_then(|error| error.api_error());
    let message = fixed_message.unwrap_or_else(|| provider_message(reply, api, body));
    api_rejection(
        op.id,
        reply.status,
        message,
        api.is_some_and(ApiError::retryable),
        &reply.credentials,
    )
}

fn api_rejection(
    id: &'static str,
    status: StatusCode,
    message: &str,
    retryable: bool,
    credentials: &SentCredentials,
) -> Error {
    let status_error = Error::from_api(ApiError::new(
        id,
        Some(status.as_u16()),
        SafeText::new("", &[]),
        SafeText::new("", &[]),
        false,
    ));
    let code = status_error.api_error().map_or("", ApiError::code);
    let class = status_error.class();
    let safe = |text| response::sanitize_error_text(text, credentials);
    let safe_status = Some(status.as_u16())
        .filter(|value| !response::reflects_credential_forms(&value.to_string(), credentials));
    let error = Error::from_api(ApiError::without_fallback(
        id,
        safe_status,
        safe(code),
        safe(message),
        retryable,
    ));
    if safe_status.is_none() && class != ErrorClass::Api {
        error.with_cause(Error::new(class))
    } else {
        error
    }
}

fn provider_message<'a>(
    reply: &'a Response,
    api: Option<&'a ApiError>,
    input: Option<&Value>,
) -> &'a str {
    static UNSAFE: LazyLock<Option<regex_lite::Regex>> = LazyLock::new(|| {
        regex_lite::Regex::new(
        r"(?i)(\b(?:password|passwd|secret|(?:access|refresh)?token|authorization|bearer|credential|cookie|session|api[-_ ]?key)\b\s*(?:[:=]|\bis\b)\s*[^\s,;]+|\bbearer\s+[^\s,;]+|\b[a-z][a-z0-9]*-[a-z0-9]*[0-9][a-z0-9-]*\b|\b[0-9]{8,}\b|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[\w.+-]+@[\w.-]+\.[a-z]{2,}|[\x00-\x1f\x7f])",
    ).ok()
    });
    let message = api.map_or_else(
        || reply.status.canonical_reason().unwrap_or(""),
        ApiError::message,
    );
    let unsafe_text = |text: &str| {
        text.len() > 512 || UNSAFE.as_ref().is_none_or(|pattern| pattern.is_match(text))
    };
    let unsafe_wire =
        crate::wire::product::evidence::Evidence::from_credentials(&reply.credentials)
            .peer_body_reflects(&reply.body)
            .unwrap_or(false)
            || serde_json::from_slice::<serde_json::Map<String, Value>>(&reply.body).is_ok_and(
                |fields| {
                    fields.iter().any(|(key, value)| {
                        key.eq_ignore_ascii_case("message")
                            && value.as_str().is_some_and(unsafe_text)
                    })
                },
            );
    let mut input_evidence = Evidence::new();
    for value in input
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|fields| {
            ["name", "package", "description"]
                .into_iter()
                .filter_map(|key| fields.get(key).and_then(Value::as_str))
        })
    {
        input_evidence.add_secret(&Secret::new(value));
    }
    if unsafe_wire || unsafe_text(message) || input_evidence.text_reflects(message) {
        "provider response withheld"
    } else {
        message
    }
}
