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
pub(crate) struct Operation {
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
#[derive(Clone, Default)]
pub(crate) struct SentCredentials {
    values: Vec<Secret>,
}
impl SentCredentials {
    #[cfg(test)]
    pub(crate) fn for_test(values: Vec<Secret>) -> Self {
        Self { values }
    }
    pub(crate) fn record(&mut self, secret: &Secret) {
        let value = secret.expose_secret();
        if !value.is_empty() && !self.values.iter().any(|item| item.expose_secret() == value) {
            self.values.push(secret.clone());
        }
    }
    fn extend(&mut self, credentials: &Self) {
        for secret in credentials.values() {
            self.record(secret);
        }
    }
    pub(crate) fn values(&self) -> &[Secret] {
        &self.values
    }
    pub(crate) fn reflects(&self, data: &[u8]) -> bool {
        self.values.iter().any(|secret| {
            let value = secret.expose_secret().as_bytes();
            !value.is_empty() && data.windows(value.len()).any(|part| part == value)
        })
    }
}
impl std::fmt::Debug for SentCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SentCredentials([redacted])")
    }
}
fn contains_secret(data: &[u8], secret: &Secret) -> bool {
    let value = secret.expose_secret().as_bytes();
    !value.is_empty() && data.windows(value.len()).any(|part| part == value)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn contains_in_component(component: &str, prior: &SentCredentials, pending: &[&Secret]) -> bool {
    let raw = component.as_bytes();
    if prior.reflects(raw) || pending.iter().any(|secret| contains_secret(raw, secret)) {
        return true;
    }
    let mut decoded = zeroize::Zeroizing::new(Vec::with_capacity(raw.len()));
    let mut cursor = 0;
    while let Some(&byte) = raw.get(cursor) {
        if byte == b'%'
            && let (Some(a), Some(b)) = (
                raw.get(cursor + 1).and_then(|b| hex(*b)),
                raw.get(cursor + 2).and_then(|b| hex(*b)),
            )
        {
            decoded.push((a << 4) | b);
            cursor += 3;
        } else {
            decoded.push(byte);
            cursor += 1;
        }
    }
    prior.reflects(&decoded)
        || pending
            .iter()
            .any(|secret| contains_secret(&decoded, secret))
}
fn uri_contains_credential(url: &Url, prior: &SentCredentials, pending: &[&Secret]) -> bool {
    contains_in_component(url.path(), prior, pending)
        || url
            .query()
            .is_some_and(|query| contains_in_component(query, prior, pending))
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
    op: &Operation,
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
pub(crate) async fn run(
    runtime: Runtime<'_>,
    op: &Operation,
    auth: Auth<'_>,
    request: &Request,
) -> Result<Response, Failure> {
    let dispatch = if matches!(op.class, redirect::Classification::Write) {
        crate::WriteDispatch::NotSent
    } else {
        crate::WriteDispatch::NotAWrite
    };
    let mut evidence = Evidence {
        status: None,
        dispatch,
    };
    let mut credentials = runtime.configured_credentials.cloned().unwrap_or_default();
    let mut sent = SentCredentials::default();
    let result = run_inner(
        runtime,
        op,
        auth,
        request,
        &mut evidence,
        &mut sent,
        &mut credentials,
    )
    .await;
    credentials.extend(&sent);
    match result {
        Ok(reply) => Ok(Response { reply, credentials }),
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
}

async fn run_inner(
    runtime: Runtime<'_>,
    op: &Operation,
    auth: Auth<'_>,
    request: &Request,
    evidence: &mut Evidence,
    sent: &mut SentCredentials,
    reflection: &mut SentCredentials,
) -> Result<send::Reply, Error> {
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
        Auth::Bearer(source) => Some(
            bounded(&ctx, clock, || source.token(&ctx))
                .await?
                .map_err(|error| Error::Provider(Box::new(error)))?,
        ),
        _ => None,
    };
    let mut recovered = false;
    let mut attempt = 0usize;
    'attempt: loop {
        if let Some(credentials) = bearer
            .as_ref()
            .and_then(|value| value.signin_credentials.as_deref())
        {
            reflection.extend(credentials);
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
                !fixed_json
                    && (sent.reflects(value.as_bytes())
                        || pending
                            .iter()
                            .any(|secret| contains_secret(value.as_bytes(), secret)))
            }) {
                return Err(Error::InvalidRequest);
            }
            if hops == 0 && uri_contains_credential(&url, sent, &pending) {
                return Err(Error::InvalidRequest);
            }
            for secret in pending {
                sent.record(secret);
            }
            let mut outgoing = http::Request::builder()
                .method(op.method.clone())
                .uri(url.as_str())
                .body(http_body_util::Full::new(request.body.clone()))
                .map_err(|_| Error::InvalidRequest)?;
            *outgoing.headers_mut() = headers;
            let mut observed = send::Evidence::default();
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
                    bearer = Some(
                        bounded(&ctx, clock, || source.token(&ctx))
                            .await?
                            .map_err(|error| Error::Provider(Box::new(error)))?,
                    );
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
                        let next = redirect::decide(
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
                        )
                        .map_err(Error::Redirect)?;
                        if uri_contains_credential(&next, sent, &[])
                            || uri_contains_credential(&next, reflection, &[])
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
            return result.map_err(Error::Send);
        }
    }
}

#[cfg(test)]
mod tests;
