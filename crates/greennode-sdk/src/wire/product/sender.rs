use super::{redirect, retry, unauthorized};
use crate::wire::{connection::Client, send};
use crate::{Clock, CredentialsProvider, RequestContext, Secret, Token};
use bytes::Bytes;
use http::{HeaderMap, Method};
use std::{future::Future, pin::Pin};
use tokio::time::Instant;
use url::Url;

pub(crate) struct BearerToken {
    pub(crate) token: Token,
    pub(crate) obtained_at: Option<unauthorized::ObtainedAt>,
}
pub(crate) trait BearerSource: Send + Sync {
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
                token: self.0.token(ctx).await?,
                obtained_at: None,
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        self.0.invalidate(ctx, sent)
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
pub(crate) struct Request {
    pub(crate) url: Url,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}
#[derive(Debug)]
pub(crate) enum Error {
    Context(crate::ContextEnd),
    Provider(Box<crate::Error>),
    Send(send::Error),
    Redirect(redirect::Refusal),
    InvalidRequest,
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

pub(crate) struct Runtime<'a> {
    pub(crate) client: &'a Client,
    pub(crate) ctx: &'a RequestContext,
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
) -> Result<send::Reply, Error> {
    let Runtime {
        client,
        ctx,
        clock,
        deadline,
        sleeper,
        jitter,
    } = runtime;
    if request.url.scheme() != "https"
        || request.url.host().is_none()
        || !request.url.username().is_empty()
        || request.url.password().is_some()
        || request.url.fragment().is_some()
        || crate::endpoint::Base::parse(&request.url[..url::Position::AfterPath]).is_err()
    {
        return Err(Error::InvalidRequest);
    }
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
        let mut url = request.url.clone();
        let mut visited = std::collections::HashSet::new();
        visited.insert(url.clone());
        let mut hops = 0usize;
        loop {
            let mut headers = request.headers.clone();
            if let Some(token) = &bearer {
                let secret = token.token.access_token.expose_secret();
                if secret.is_empty()
                    || secret
                        .bytes()
                        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
                {
                    return Err(Error::InvalidRequest);
                }
                let text = zeroize::Zeroizing::new(format!("Bearer {secret}"));
                let mut value =
                    http::HeaderValue::from_str(&text).map_err(|_| Error::InvalidRequest)?;
                value.set_sensitive(true);
                headers.insert(http::header::AUTHORIZATION, value);
            }
            let mut outgoing = http::Request::builder()
                .method(op.method.clone())
                .uri(url.as_str())
                .body(http_body_util::Full::new(request.body.clone()))
                .map_err(|_| Error::InvalidRequest)?;
            *outgoing.headers_mut() = headers;
            let result = bounded(&ctx, clock, || {
                send::once(client, outgoing, op.cap, op.encoding, clock, deadline)
            })
            .await?;
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
