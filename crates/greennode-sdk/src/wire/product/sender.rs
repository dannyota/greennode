pub(crate) use super::evidence::SentCredentials;
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

mod uri;
pub(super) use uri::uri_reflects_credential_forms;
use uri::{contains_secret, uri_contains_credential};

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
    pub(crate) pacing: Option<&'a super::options::Pacing>,
    pub(crate) response_guard: Option<(&'static str, ResponseGuard)>,
    pub(crate) attempt_check: Option<super::options::AttemptCheck<'a>>,
    pub(crate) method: Method,
    pub(crate) class: redirect::Classification,
    pub(crate) retry: redirect::Retry,
    pub(crate) no_redirect: bool,
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
fn credential_value(secret: &Secret, bearer: bool) -> Result<http::HeaderValue, Error> {
    let value = secret.expose_secret();
    if value.is_empty()
        || (bearer
            && value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control()))
    {
        return Err(Error::InvalidRequest);
    }
    let text = zeroize::Zeroizing::new(if bearer {
        format!("Bearer {value}")
    } else {
        value.to_owned()
    });
    let mut header = http::HeaderValue::from_str(&text).map_err(|_| Error::InvalidRequest)?;
    header.set_sensitive(true);
    Ok(header)
}

pub(crate) struct Response {
    pub(crate) reply: send::Reply,
    pub(crate) credentials: SentCredentials,
    pub(crate) reflection: Option<bool>,
}
impl std::ops::Deref for Response {
    type Target = send::Reply;
    fn deref(&self) -> &Self::Target {
        &self.reply
    }
}
#[derive(Debug)]
pub(crate) enum Error {
    Context(crate::ContextEnd),
    Provider(Box<crate::Error>),
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
    pub(crate) refused_body: Vec<u8>,
}
impl Failure {
    pub(crate) fn into_failure<T>(self, error: crate::Error) -> crate::Failure<T> {
        let error = match self.error {
            Error::Provider(provider) => *provider,
            _ => error,
        };
        let error = match self.status {
            Some(status) => error.with_outer_detail(status),
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

fn system_time(time: time::OffsetDateTime) -> Option<std::time::SystemTime> {
    let nanos = time.unix_timestamp_nanos();
    let absolute = nanos.unsigned_abs();
    let duration = std::time::Duration::new(
        u64::try_from(absolute / 1_000_000_000).ok()?,
        u32::try_from(absolute % 1_000_000_000).ok()?,
    );
    if nanos >= 0 {
        std::time::SystemTime::UNIX_EPOCH.checked_add(duration)
    } else {
        std::time::SystemTime::UNIX_EPOCH.checked_sub(duration)
    }
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
#[derive(Default)]
pub(super) struct RouteInputs<'a> {
    pub(super) path: &'a [&'a str],
    pub(super) query: &'a [(&'a str, &'a str)],
    pub(super) password_slot: Option<super::options::PasswordSlot<'a>>,
    pub(super) identifier_query_keys: &'a [&'a str],
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
        refused_body: Vec::new(),
    };
    let result = run_inner(runtime, op, auth, request, route_inputs, &mut evidence).await;
    let mut credentials = evidence.reflection;
    credentials.extend(&evidence.sent);
    super::exit::record(&credentials, &evidence.sent, evidence.dispatch);
    match result {
        Ok((reply, reflection)) => Ok(Response {
            reply,
            credentials,
            reflection,
        }),
        Err(error) => Err(Failure {
            error,
            status: evidence.status,
            dispatch: evidence.dispatch,
            credentials,
            refused_body: evidence.refused_body,
        }),
    }
}

struct Evidence {
    status: Option<http::StatusCode>,
    dispatch: crate::WriteDispatch,
    sent: SentCredentials,
    reflection: SentCredentials,
    refused_body: Vec<u8>,
}

async fn run_inner(
    runtime: Runtime<'_>,
    op: &Operation<'_>,
    auth: Auth<'_>,
    request: &Request,
    route_inputs: RouteInputs<'_>,
    evidence: &mut Evidence,
) -> Result<(send::Reply, Option<bool>), Error> {
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
            if route_inputs.reflects(credentials) {
                return Err(Error::InvalidRequest);
            }
        }
        evidence.status = None;
        if let Some(check) = op.attempt_check
            && let Some(error) = (check.0)(reflection)
        {
            return Err(Error::Provider(error));
        }
        let mut url = request.url.clone();
        let mut visited = std::collections::HashSet::new();
        visited.insert(url.clone());
        let mut hops = 0usize;
        loop {
            evidence.status = None;
            let mut headers = request.headers.clone();
            let mut pending = Vec::new();
            for credential in &request.credentials {
                if headers.contains_key(&credential.name) {
                    return Err(Error::InvalidRequest);
                }
                let value = credential_value(&credential.value, credential.bearer)?;
                headers.insert(credential.name.clone(), value);
                pending.push(&credential.value);
            }
            if let Some(token) = &bearer {
                if headers.contains_key(http::header::AUTHORIZATION) {
                    return Err(Error::InvalidRequest);
                }
                let secret = &token.token.access_token;
                headers.insert(http::header::AUTHORIZATION, credential_value(secret, true)?);
                pending.push(secret);
            }
            if request.headers.iter().any(|(name, value)| {
                let fixed_json = (name == http::header::ACCEPT
                    || name == http::header::CONTENT_TYPE)
                    && value.as_bytes() == b"application/json";
                let fixed_html = matches!(auth, Auth::None)
                    && name == http::header::ACCEPT
                    && value.as_bytes() == b"text/html";
                !(fixed_json || fixed_html)
                    && (std::str::from_utf8(value.as_bytes()).is_ok_and(|text| {
                        super::response::reflects_credential_forms(text, reflection)
                            || super::response::reflects_credential_forms(text, sent)
                    }) || reflection.reflects(value.as_bytes())
                        || sent.reflects(value.as_bytes())
                        || pending
                            .iter()
                            .any(|secret| contains_secret(value.as_bytes(), secret)))
            }) {
                return Err(Error::InvalidRequest);
            }
            if hops == 0 && uri_contains_credential(&url, sent, &pending) {
                return Err(Error::InvalidRequest);
            }
            let mut credentials = reflection.transport_only();
            credentials.extend(sent);
            for secret in &pending {
                credentials.record(secret);
            }
            let reflected_route = route_inputs.reflects(&credentials);
            if reflected_route {
                reflection.extend(&credentials);
            }
            if reflected_route
                || super::evidence::body_reflects(
                    &request.body,
                    &credentials,
                    route_inputs.password_slot,
                )
            {
                return Err(Error::Provider(Box::new(crate::Error::new(
                    crate::ErrorClass::InvalidInput,
                ))));
            }
            if let Some(pacing) = op.pacing {
                bounded(&ctx, clock, || pacing.before_send(&ctx, clock))
                    .await?
                    .map_err(Error::Context)?;
            }
            for secret in pending {
                sent.record_product_credential(secret);
            }
            let mut outgoing = http::Request::builder()
                .method(op.method.clone())
                .uri(url.as_str())
                .body(http_body_util::Full::new(request.body.clone()))
                .map_err(|_| Error::InvalidRequest)?;
            *outgoing.headers_mut() = headers;
            let mut observed = send::Evidence::default();
            #[cfg(test)]
            if matches!(op.class, redirect::Classification::Write) {
                observed.attempt_counter = Some(sleeper);
            }
            let result = bounded(&ctx, clock, || {
                send::once_with_evidence(
                    client,
                    outgoing,
                    op.cap,
                    op.encoding,
                    clock,
                    deadline,
                    &mut observed,
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
                    Ok(Err(send::Error::Transport(retry::SendFailure::FailedDial)))
                )
            {
                evidence.dispatch = crate::WriteDispatch::MayHaveBeenSent;
            }
            let result = result?;
            let body_reflection = result.as_ref().ok().and_then(|reply| {
                let mut evidence = super::evidence::Evidence::from_credentials(reflection);
                evidence.absorb(sent);
                evidence.go_peer_body_reflects(&reply.body)
            });
            if let (Some((id, guard)), Ok(reply)) = (op.response_guard, &result)
                && let Some(error) = guard(id, reply.status, &reply.body, reflection, sent)
            {
                return Err(Error::Provider(error));
            }
            let is_401 = match &result {
                Ok(reply) => reply.status == http::StatusCode::UNAUTHORIZED,
                Err(error) => error
                    .observed()
                    .is_some_and(|head| head.status == http::StatusCode::UNAUTHORIZED),
            };
            if is_401 {
                let decision = unauthorized_decision(
                    auth,
                    bearer.as_ref(),
                    op,
                    result.is_err(),
                    recovered,
                    clock,
                );
                if let (Auth::Bearer(source), Some(sent)) = (auth, decision.rejected) {
                    bounded(&ctx, clock, || {
                        source.rejected(&ctx, sent, decision.invalidate.is_some())
                    })
                    .await?
                    .map_err(|error| Error::Provider(Box::new(error)))?;
                }
                if decision.resend
                    && let Auth::Bearer(source) = auth
                    && source.refreshable()
                {
                    bearer = Some(acquire_bearer(&ctx, clock, source, reflection).await?);
                    recovered = true;
                    attempt = 0;
                    continue 'attempt;
                }
            }
            let outcome = match &result {
                Ok(reply) => {
                    if matches!(reply.status.as_u16(), 301 | 302 | 303 | 307 | 308)
                        && let Some(location) = reply.headers.get(http::header::LOCATION)
                    {
                        let location = location.to_str().map_err(|_| Error::InvalidRequest)?;
                        let next = match redirect::decide(
                            &url,
                            redirect::Operation {
                                class: op.class,
                                method: &op.method,
                                retry: op.retry,
                                no_redirect: op.no_redirect,
                            },
                            hops,
                            &visited,
                            location,
                        ) {
                            Ok(next) => next,
                            Err(refusal) => {
                                evidence.refused_body = reply.body.clone();
                                return Err(Error::Redirect(refusal));
                            }
                        };
                        credentials.extend(reflection);
                        let reflected = match auth {
                            Auth::Bearer(_) => {
                                super::response::reflects_credential(location, &credentials)
                            }
                            _ => uri_reflects_credential_forms(&next, &credentials),
                        };
                        let reflected_path =
                            next.path_segments().into_iter().flatten().any(|segment| {
                                super::response::reflects_identifier(segment, &credentials)
                            });
                        let reflected_query = next.query().is_some_and(|query| {
                            query.split('&').any(|pair| {
                                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                                [key, value].into_iter().any(|part| {
                                    super::response::reflects_identifier(part, &credentials)
                                })
                            })
                        });
                        if reflected
                            || reflected_path
                            || reflected_query
                            || uri_contains_credential(&next, &credentials, &[])
                        {
                            return Err(Error::Redirect(redirect::Refusal::Invalid));
                        }
                        hops += 1;
                        visited.insert(next.clone());
                        url = next;
                        continue;
                    }
                    let hint = reply
                        .headers
                        .get(http::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok());
                    Some(retry::Outcome::Status(reply.status.as_u16(), hint))
                }
                Err(error) => error.retry_outcome(),
            };
            let now = system_time(clock.utc_now()).ok_or(Error::InvalidRequest)?;
            if let Some(delay) = outcome.and_then(|outcome| {
                retry::decide(op.retry, attempt, &op.retry_config, outcome, now, jitter())
            }) {
                bounded(&ctx, clock, || sleeper.sleep(delay)).await?;
                attempt += 1;
                continue 'attempt;
            }
            return result
                .map(|reply| (reply, body_reflection))
                .map_err(Error::Send);
        }
    }
}

#[cfg(test)]
mod tests;
