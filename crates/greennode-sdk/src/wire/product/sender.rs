pub(crate) use super::evidence::SentCredentials;
use super::response::{ProjectDiscovery, Screen};
use super::{redirect, retry, unauthorized};
use crate::wire::{
    connection::{OneSendClient, PooledClient},
    send,
};
use crate::{Clock, CredentialsProvider, RequestContext, Secret, Token};
use bytes::Bytes;
use http::{HeaderMap, Method};
use std::{future::Future, pin::Pin};
use tokio::time::Instant;
use url::Url;

mod credentials;
mod response_body;
mod uri;
use credentials::{active_request_context, attempt_headers, check_history};
use uri::redirect_target;
use uri::uri_contains_credential;
pub(super) use uri::uri_reflects_credential_forms;

pub(crate) struct BearerToken {
    pub(crate) token: Token,
    pub(crate) obtained_at: Option<unauthorized::ObtainedAt>,
    pub(crate) signin_credentials: Option<std::sync::Arc<SentCredentials>>,
}
pub(crate) trait BearerSource: Send + Sync {
    fn refreshable(&self) -> bool {
        true
    }
    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, crate::Error>> + Send + 'a>>;
    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>>;
}
pub(crate) struct PublicSource<'a>(pub(crate) &'a dyn CredentialsProvider);
impl BearerSource for PublicSource<'_> {
    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(BearerToken {
                token: self
                    .0
                    .token(ctx)
                    .await
                    .map_err(|_| crate::Error::provider_failed())?,
                obtained_at: None,
                signin_credentials: None,
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.0
                .invalidate(ctx, sent)
                .await
                .map_err(|_| crate::Error::provider_failed())
        })
    }
}
#[derive(Clone, Copy)]
pub(crate) enum Auth<'a> {
    Bearer(&'a dyn BearerSource),
    Key,
    None,
}
pub(crate) type ResponseGuard = fn(
    &'static str,
    http::StatusCode,
    &[u8],
    &SentCredentials,
    &SentCredentials,
) -> Option<Box<crate::Error>>;

pub(crate) struct Operation<'a> {
    pub(crate) guard: Option<&'static str>,
    pub(crate) pacing: Option<&'a super::options::Pacing>,
    pub(crate) project_discovery: Option<ProjectDiscovery>,
    pub(crate) response_guard: Option<(&'static str, ResponseGuard)>,
    pub(crate) attempt_check: Option<super::options::AttemptCheck<'a>>,
    pub(crate) method: Method,
    pub(crate) class: redirect::Classification,
    pub(crate) retry: redirect::Retry,
    pub(crate) no_redirect: bool,
    pub(crate) report_redirect: bool,
    pub(crate) keep_status: bool,
    pub(crate) allow_recovery: bool,
    pub(crate) cap: usize,
    pub(crate) encoding: send::Encoding,
    pub(crate) retry_config: retry::Config,
}
pub(crate) struct CredentialHeader {
    pub(crate) name: http::HeaderName,
    pub(crate) value: Secret,
    pub(crate) bearer: bool,
}
pub(crate) struct Request {
    pub(crate) url: Url,
    pub(crate) headers: HeaderMap,
    pub(crate) credentials: Vec<CredentialHeader>,
    pub(crate) body: Bytes,
}
#[must_use = "inspect the receipt or explicitly discard a screened acknowledgement"]
pub(crate) struct Response {
    pub(crate) status: http::StatusCode,
    pub(crate) headers: HeaderMap,
    body: zeroize::Zeroizing<Vec<u8>>,
    body_cap: usize,
    pub(crate) credentials: SentCredentials,
    pub(crate) reflection: Screen,
    receipt_screen: Screen,
    receipt_policy: super::guard::ReceiptPolicy,
    project_output: bool,
    write_response: bool,
    transport_screen: std::sync::OnceLock<Screen>,
    pub(super) request: Option<super::absence::RequestIdentity>,
}
crate::error::debug_record!(Response unit);

#[derive(Debug)]
pub(crate) enum Error {
    Context(crate::ContextEnd),
    ActiveRequestContext(crate::ContextEnd),
    BodyContext(crate::ContextEnd),
    Provider(Box<crate::Error>),
    Invalidation(Box<crate::Error>),
    Send(send::Error),
    Redirect(redirect::Refusal),
    InvalidRequest,
}
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) error: Error,
    pub(crate) status: Option<http::StatusCode>,
    pub(crate) dispatch: crate::WriteDispatch,
    pub(crate) credentials: SentCredentials,
}
impl Failure {
    pub(crate) fn into_failure<T>(self, error: crate::Error) -> crate::Failure<T> {
        let error = match self.error {
            Error::Provider(provider) | Error::Invalidation(provider) => *provider,
            _ => error,
        };
        let error = match self.status {
            Some(status) => error.with_http_status(status),
            None => error,
        };
        crate::Failure::new(error, None, self.dispatch)
    }
}

pub(crate) trait Sleeper: Send + Sync {
    fn sleep(&self, duration: std::time::Duration)
    -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    #[cfg(test)]
    fn record_write_attempt(&self) {}
}
async fn bounded<T, F: Future<Output = T>>(
    ctx: &RequestContext,
    clock: &dyn Clock,
    make_future: impl FnOnce() -> F,
) -> Result<T, Error> {
    if let Some(end) = ctx.check(clock) {
        return Err(Error::Context(end));
    }
    let mut end = Box::pin(ctx.wait(clock));
    let mut future = Box::pin(make_future());
    std::future::poll_fn(|cx| {
        if let Some(end) = ctx.check(clock) {
            return std::task::Poll::Ready(Err(Error::Context(end)));
        }
        if let std::task::Poll::Ready(end) = end.as_mut().poll(cx) {
            return std::task::Poll::Ready(Err(Error::Context(end)));
        }
        match future.as_mut().poll(cx) {
            std::task::Poll::Ready(value) => std::task::Poll::Ready(match ctx.check(clock) {
                Some(end) => Err(Error::Context(end)),
                None => Ok(value),
            }),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    })
    .await
}

async fn acquire_bearer(
    ctx: &RequestContext,
    clock: &dyn Clock,
    source: &dyn BearerSource,
    credentials: &mut SentCredentials,
) -> Result<BearerToken, Error> {
    let mut started = false;
    let result = bounded(ctx, clock, || {
        started = true;
        source.token(ctx)
    })
    .await
    .and_then(|result| result.map_err(|error| Error::Provider(Box::new(error))))
    .and_then(
        |bearer| match bearer.token.access_token.expose_secret().is_empty() {
            true => Err(Error::Provider(Box::new(unauthorized::no_token()))),
            false => Ok(bearer),
        },
    )
    .inspect(|bearer| credentials.record(&bearer.token.access_token));
    if started && result.is_err() {
        credentials.mark_incomplete_signin();
    }
    result
}

fn unauthorized_decision<'a>(
    auth: Auth<'_>,
    bearer: Option<&'a BearerToken>,
    op: &Operation<'_>,
    body_error: bool,
    recovered: bool,
    clock: &dyn Clock,
) -> unauthorized::Decision<'a> {
    let current = match (auth, bearer) {
        (Auth::Bearer(_), Some(bearer)) => {
            let obtained_at = match &bearer.obtained_at {
                Some(unauthorized::ObtainedAt::InProcess(t)) => {
                    unauthorized::ObtainedAt::InProcess(*t)
                }
                Some(unauthorized::ObtainedAt::Disk(t)) => unauthorized::ObtainedAt::Disk(*t),
                None => unauthorized::ObtainedAt::InProcess(clock.monotonic_now()),
            };
            unauthorized::Auth::Bearer {
                sent: &bearer.token.access_token,
                obtained_at,
                built_in: bearer.obtained_at.is_some(),
            }
        }
        (Auth::Key, _) => unauthorized::Auth::Key,
        _ => unauthorized::Auth::Unauthenticated,
    };
    unauthorized::decide(
        current,
        op.retry,
        op.allow_recovery,
        body_error,
        recovered,
        unauthorized::Now {
            monotonic: clock.monotonic_now(),
            wall: clock.utc_now(),
        },
    )
}

pub(crate) struct Clients {
    pub(crate) pooled: PooledClient,
    pub(crate) one_send: OneSendClient,
}

pub(crate) struct Runtime<'a> {
    pub(crate) client: &'a Clients,
    pub(crate) ctx: &'a RequestContext,
    pub(crate) configured_credentials: Option<&'a SentCredentials>,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) deadline: Instant,
    pub(crate) sleeper: &'a dyn Sleeper,
    pub(crate) jitter: &'a mut (dyn FnMut() -> u64 + Send),
}
#[derive(Clone, Copy, Default)]
pub(super) struct RouteInputs<'a> {
    pub(super) path: &'a [&'a str],
    pub(super) query: &'a [(&'a str, &'a str)],
    pub(super) password_slot: Option<super::options::PasswordSlot<'a>>,
    pub(super) identifier_query_keys: &'a [&'a str],
    pub(super) descriptor: Option<(&'static str, &'static str)>,
    pub(super) project_output: bool,
    pub(super) redact_output: bool,
    pub(super) receipt_policy: super::guard::ReceiptPolicy,
}

impl RouteInputs<'_> {
    fn reflects(&self, credentials: &SentCredentials) -> bool {
        self.path
            .iter()
            .any(|id| super::response::reflects_identifier(id, credentials))
            || self.query.iter().any(|(key, value)| {
                if self.identifier_query_keys.contains(key) {
                    super::response::reflects_identifier(value, credentials)
                } else {
                    super::response::reflects_credential(value, credentials)
                }
            })
    }
}

pub(super) async fn run(
    runtime: Runtime<'_>,
    op: &Operation<'_>,
    auth: Auth<'_>,
    request: &Request,
    route_inputs: RouteInputs<'_>,
) -> Result<Response, Failure> {
    let dispatch = if matches!(op.class, redirect::Classification::Write) {
        crate::WriteDispatch::NotSent
    } else {
        crate::WriteDispatch::NotAWrite
    };
    let mut evidence = Evidence {
        status: None,
        dispatch,
        sent: SentCredentials::default(),
        reflection: runtime.configured_credentials.cloned().unwrap_or_default(),
    };
    let result = run_inner(runtime, op, auth, request, route_inputs, &mut evidence).await;
    let mut credentials = evidence.reflection;
    credentials.extend(&evidence.sent);
    super::exit::record(&credentials, &evidence.sent, evidence.dispatch);
    match result {
        Ok((reply, reflection, answered)) => Ok(Response::from_exchange(
            reply,
            reflection,
            credentials,
            op,
            &request.url,
            &answered,
            route_inputs,
        )),
        Err(error) => Err(Failure {
            error,
            status: evidence.status,
            dispatch: evidence.dispatch,
            credentials,
        }),
    }
}

struct Evidence {
    status: Option<http::StatusCode>,
    dispatch: crate::WriteDispatch,
    sent: SentCredentials,
    reflection: SentCredentials,
}

async fn run_inner(
    runtime: Runtime<'_>,
    op: &Operation<'_>,
    auth: Auth<'_>,
    request: &Request,
    route_inputs: RouteInputs<'_>,
    evidence: &mut Evidence,
) -> Result<(send::Reply, Screen, Url), Error> {
    let sent = &mut evidence.sent;
    let reflection = &mut evidence.reflection;
    let Runtime {
        client,
        ctx,
        clock,
        deadline,
        sleeper,
        jitter,
        configured_credentials: _,
    } = runtime;
    check_history(&[reflection])?;
    if request.url.scheme() != "https"
        || request.url.host().is_none()
        || !request.url.username().is_empty()
        || request.url.password().is_some()
        || request.url.fragment().is_some()
        || crate::endpoint::Base::parse(&request.url[..url::Position::AfterPath]).is_err()
        || request.headers.iter().any(|(name, value)| {
            value.is_sensitive()
                || matches!(
                    name.as_str(),
                    "authorization"
                        | "proxy-authorization"
                        | "cookie"
                        | "x-api-key"
                        | "api-key"
                        | "x-auth-token"
                        | "x-access-token"
                        | "x-api-token"
                )
        })
        || (matches!(auth, Auth::None) && !request.credentials.is_empty())
        || (matches!(auth, Auth::Key) && request.credentials.is_empty())
        || route_inputs.reflects(&reflection.transport_only())
    {
        return Err(Error::InvalidRequest);
    }
    let client = if matches!(op.class, redirect::Classification::Read)
        && matches!(op.retry, redirect::Retry::Repeatable)
    {
        client.pooled.as_client()
    } else {
        client.one_send.as_client()
    };
    let ctx = ctx.child_with_deadline(deadline);
    let deadline = ctx.deadline().ok_or(Error::InvalidRequest)?;
    let mut bearer = match auth {
        Auth::Bearer(source) => Some(acquire_bearer(&ctx, clock, source, reflection).await?),
        _ => None,
    };
    let mut recovered = false;
    let mut attempt = 0usize;
    'attempt: loop {
        if let Some(token) = &bearer {
            reflection.record_product_credential(&token.token.access_token);
            reflection.record_bearer(&token.token.access_token);
        }
        if let Some(credentials) = bearer
            .as_ref()
            .and_then(|value| value.signin_credentials.as_deref())
        {
            reflection.extend(credentials);
            sent.extend(credentials);
        }
        check_history(&[reflection, sent])?;
        evidence.status = None;
        let (headers, header_credentials) = attempt_headers(request, bearer.as_ref())?;
        let mut outgoing_credentials = sent.clone();
        outgoing_credentials.extend(&header_credentials);
        reflection.extend(&header_credentials);
        let mut credentials = reflection.clone();
        credentials.extend(&outgoing_credentials);
        check_history(&[&credentials])?;
        if let Some(check) = op.attempt_check
            && let Some(error) = (check.0)(&credentials)
        {
            return Err(Error::Provider(error));
        }
        if request.headers.iter().any(|(name, value)| {
            let fixed_json = (name == http::header::ACCEPT || name == http::header::CONTENT_TYPE)
                && value.as_bytes() == b"application/json";
            let fixed_html = matches!(auth, Auth::None)
                && name == http::header::ACCEPT
                && value.as_bytes() == b"text/html";
            !(fixed_json || fixed_html)
                && match std::str::from_utf8(value.as_bytes()) {
                    Ok(text) => super::response::reflects_credential_forms(text, &credentials),
                    Err(_) => credentials.reflects(value.as_bytes()),
                }
        }) || uri_contains_credential(&request.url, &outgoing_credentials)
        {
            return Err(Error::InvalidRequest);
        }
        let transport = credentials.transport_only();
        if route_inputs.reflects(&transport)
            || super::evidence::body_reflects(&request.body, &transport, route_inputs.password_slot)
        {
            return Err(Error::Provider(Box::new(crate::Error::new(
                crate::ErrorClass::InvalidInput,
            ))));
        }
        let mut url = request.url.clone();
        let mut visited = std::collections::HashSet::new();
        visited.insert(url.clone());
        let mut hops = 0usize;
        loop {
            evidence.status = None;
            if let Some(pacing) = op.pacing {
                bounded(&ctx, clock, || pacing.before_send(&ctx, clock))
                    .await?
                    .map_err(Error::Context)?;
            }
            let mut outgoing = http::Request::builder()
                .method(op.method.clone())
                .uri(url.as_str())
                .body(http_body_util::Full::new(request.body.clone()))
                .map_err(|_| Error::InvalidRequest)?;
            *outgoing.headers_mut() = headers.clone();
            let mut observed = send::Evidence::default();
            #[cfg(test)]
            if matches!(op.class, redirect::Classification::Write) {
                observed.attempt_counter = Some(sleeper);
            }
            let mut redirect = None;
            let read_body = |status: http::StatusCode, headers: &HeaderMap| {
                if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                    return true;
                }
                if matches!(status.as_u16(), 307 | 308)
                    && matches!(op.retry, redirect::Retry::Once)
                    && op.report_redirect
                {
                    return true;
                }
                let Some(location) = headers
                    .get(http::header::LOCATION)
                    .filter(|value| !value.is_empty())
                else {
                    return true;
                };
                let decision =
                    redirect_target(&url, op, hops, &visited, location, auth, &credentials);
                if op.report_redirect && matches!(decision, Err(redirect::Refusal::Operation)) {
                    return true;
                }
                redirect = Some(decision);
                false
            };
            let result = bounded(&ctx, clock, || {
                sent.extend(&header_credentials);
                if let Some(slot) = route_inputs.password_slot {
                    super::exit::observe_operation_password(slot);
                }
                send::with_sent_credentials(
                    sent,
                    send::once_with_evidence(
                        client,
                        outgoing,
                        op.cap,
                        op.encoding,
                        clock,
                        deadline,
                        (&mut observed, read_body),
                    ),
                )
            })
            .await;
            if let Some(status) = observed.status {
                evidence.status = Some(status);
            }
            if evidence.dispatch == crate::WriteDispatch::NotSent
                && observed.started
                && !matches!(
                    result,
                    Ok(Err(send::Error::Transport(
                        retry::SendFailure::FailedDial,
                        _
                    )))
                )
            {
                evidence.dispatch = crate::WriteDispatch::MayHaveBeenSent;
            }
            let result = result.map_err(|error| match error {
                Error::Context(end) if observed.status.is_some() => Error::BodyContext(end),
                Error::Context(end) if observed.attempted => Error::ActiveRequestContext(end),
                _ => error,
            })?;
            if let Some(redirect) = redirect {
                match redirect {
                    Ok(target) => {
                        url = target;
                        hops += 1;
                        visited.insert(url.clone());
                        continue;
                    }
                    Err(refusal) => {
                        if op.method == Method::GET
                            && matches!(op.retry, redirect::Retry::Repeatable)
                            && let Some(delay) = retry::decide(
                                op.retry,
                                attempt,
                                &op.retry_config,
                                retry::Outcome::Send(retry::SendFailure::PossiblySent),
                                retry::system_time(clock.utc_now()).ok_or(Error::InvalidRequest)?,
                                jitter(),
                            )
                        {
                            evidence.status = None;
                            bounded(&ctx, clock, || sleeper.sleep(delay))
                                .await
                                .map_err(active_request_context)?;
                            attempt += 1;
                            continue 'attempt;
                        }
                        return Err(Error::Redirect(refusal));
                    }
                }
            }
            let body_reflection = result.as_ref().map_or(Screen::Indeterminate, |reply| {
                op.screen_reply(matches!(auth, Auth::None), reply, reflection, sent)
            });
            let refused = op.guard.filter(|_| {
                op.project_discovery.is_none() && result.is_ok() && !body_reflection.is_clean()
            });
            let callback_refusal = if refused.is_none()
                && let Ok(reply) = &result
            {
                op.refuse_before_retry(reply, body_reflection, reflection, sent)
            } else {
                None
            };
            let is_401 = match &result {
                Ok(reply) => reply.status == http::StatusCode::UNAUTHORIZED,
                Err(error) => {
                    (op.keep_status || matches!(error.kind(), send::Error::BodyTooLarge))
                        && error
                            .observed()
                            .is_some_and(|head| head.status == http::StatusCode::UNAUTHORIZED)
                }
            };
            if is_401 {
                let decision = unauthorized_decision(
                    auth,
                    bearer.as_ref(),
                    op,
                    result.is_err() || refused.is_some() || callback_refusal.is_some(),
                    recovered,
                    clock,
                );
                let resending =
                    decision.resend && matches!(auth, Auth::Bearer(source) if source.refreshable());
                if let (Auth::Bearer(source), Some(sent)) = (auth, decision.rejected) {
                    let rejected = match bounded(&ctx, clock, || {
                        source.rejected(&ctx, sent, decision.invalidate.is_some())
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(Error::Context(end)) => Err(end.into()),
                        Err(error) => return Err(error),
                    };
                    rejected.map_err(|error| {
                        if resending {
                            Error::Provider(Box::new(error))
                        } else {
                            Error::Invalidation(Box::new(error))
                        }
                    })?;
                }
                if resending && let Auth::Bearer(source) = auth {
                    evidence.status = None;
                    bearer = Some(acquire_bearer(&ctx, clock, source, reflection).await?);
                    recovered = true;
                    attempt = 0;
                    continue 'attempt;
                }
            }
            if let Some(id) = refused {
                let observed = evidence.status;
                evidence.status = body_reflection.diagnostic_status(observed);
                let error = super::absence::keep_unscreened_status(
                    super::operation::unsafe_guard_response(id),
                    observed,
                );
                let error = if body_reflection == Screen::Indeterminate
                    && let Ok(reply) = &result
                    && serde_json::from_str::<&serde_json::value::RawValue>(
                        &crate::model::wire::go_json(&reply.body),
                    )
                    .is_err()
                {
                    error.with_outer_detail(super::operation::MalformedGuard)
                } else {
                    error
                };
                return Err(Error::Provider(Box::new(error)));
            }
            if let Some(error) = callback_refusal {
                return Err(Error::Provider(error));
            }
            let outcome = match &result {
                Ok(reply) => {
                    let hint = reply
                        .headers
                        .get(http::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok());
                    Some(retry::Outcome::Status(reply.status.as_u16(), hint))
                }
                Err(error) => error.retry_outcome(),
            };
            let now = retry::system_time(clock.utc_now()).ok_or(Error::InvalidRequest)?;
            if let Some(delay) = outcome.and_then(|outcome| {
                retry::decide(op.retry, attempt, &op.retry_config, outcome, now, jitter())
            }) {
                bounded(&ctx, clock, || sleeper.sleep(delay))
                    .await
                    .map_err(active_request_context)?;
                attempt += 1;
                continue 'attempt;
            }
            return result
                .map(|reply| {
                    let receipt_screen =
                        op.receipt_screen(&reply, body_reflection, reflection, sent);
                    (reply, receipt_screen, url)
                })
                .map_err(Error::Send);
        }
    }
}

#[cfg(test)]
mod tests;
