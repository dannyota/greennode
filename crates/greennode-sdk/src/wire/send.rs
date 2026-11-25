use super::connection::Client;
use crate::Clock;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper::body::Body;
use hyper_util::client::legacy::connect::{CaptureConnection, capture_connection};
use std::{
    fmt,
    future::{Future, poll_fn},
    task::Poll,
};
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Encoding {
    Identity,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidRequest,
    BodyTooLarge,
    Deadline,
    SwitchingProtocols,
    Encoding,
    Transport,
    Body,
    AfterHead { reason: Box<Error>, head: Observed },
}

#[derive(PartialEq, Eq)]
pub(crate) struct Observed {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
}

impl fmt::Debug for Observed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observed")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Error {
    pub(crate) fn observed(&self) -> Option<&Observed> {
        match self {
            Self::AfterHead { head, .. } => Some(head),
            _ => None,
        }
    }
    pub(crate) fn kind(&self) -> &Self {
        match self {
            Self::AfterHead { reason, .. } => reason.kind(),
            _ => self,
        }
    }
    fn with_head(self, head: Observed) -> Self {
        Self::AfterHead {
            reason: Box::new(self),
            head,
        }
    }
}

pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

struct RefusalGuard {
    capture: CaptureConnection,
    complete: bool,
}
impl Drop for RefusalGuard {
    fn drop(&mut self) {
        if !self.complete
            && let Some(connection) = self.capture.connection_metadata().as_ref()
        {
            connection.poison();
        }
    }
}

async fn timed<T>(
    clock: &dyn Clock,
    deadline: Instant,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    let mut future = Box::pin(future);
    let mut timer = clock.sleep_until(deadline);
    poll_fn(|cx| {
        if clock.monotonic_now() >= deadline || timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Error::Deadline));
        }
        match future.as_mut().poll(cx) {
            Poll::Ready(value) => Poll::Ready(Ok(value)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

pub(crate) async fn once(
    client: &Client,
    mut request: Request<Full<Bytes>>,
    cap: usize,
    encoding: Encoding,
    clock: &dyn Clock,
    deadline: Instant,
) -> Result<Reply, Error> {
    let method = request.method();
    if !matches!(
        *method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) || request.uri().scheme_str() != Some("https")
        || request.uri().authority().is_none()
        || request
            .uri()
            .authority()
            .is_some_and(|a| a.as_str().contains('@'))
        || request.headers().contains_key(header::UPGRADE)
        || request
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .any(|value| {
                value.to_str().map_or(true, |s| {
                    s.split(',')
                        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
                })
            })
    {
        return Err(Error::InvalidRequest);
    }
    if clock.monotonic_now() >= deadline {
        return Err(Error::Deadline);
    }
    let mut guard = RefusalGuard {
        capture: capture_connection(&mut request),
        complete: false,
    };
    let is_head = request.method() == Method::HEAD;
    let response = timed(clock, deadline, client.request(request))
        .await?
        .map_err(|_| Error::Transport)?;
    let (parts, mut body) = response.into_parts();
    let head = Observed {
        status: parts.status,
        headers: parts.headers,
    };
    if clock.monotonic_now() >= deadline {
        return Err(Error::Deadline.with_head(head));
    }
    if head.status == StatusCode::SWITCHING_PROTOCOLS {
        return Err(Error::SwitchingProtocols.with_head(head));
    }
    match encoding {
        Encoding::Identity => {
            if head
                .headers
                .get_all(header::CONTENT_ENCODING)
                .iter()
                .any(|value| {
                    value.to_str().map_or(true, |s| {
                        s.split(',')
                            .any(|token| !token.trim().eq_ignore_ascii_case("identity"))
                    })
                })
            {
                return Err(Error::Encoding.with_head(head));
            }
        }
    }
    if !is_head
        && !head.status.is_informational()
        && head.status != StatusCode::NO_CONTENT
        && head.status != StatusCode::NOT_MODIFIED
        && body
            .size_hint()
            .exact()
            .is_some_and(|n| n > u64::try_from(cap).unwrap_or(u64::MAX))
    {
        return Err(Error::BodyTooLarge.with_head(head));
    }
    let read = async {
        let mut bytes = Vec::new();
        loop {
            if clock.monotonic_now() >= deadline {
                return Err(Error::Deadline);
            }
            let Some(frame) = body.frame().await else {
                break;
            };
            if clock.monotonic_now() >= deadline {
                return Err(Error::Deadline);
            }
            let frame = frame.map_err(|_| Error::Body)?;
            if let Ok(data) = frame.into_data() {
                if data.get(cap.saturating_sub(bytes.len())).is_some() {
                    return Err(Error::BodyTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        if clock.monotonic_now() >= deadline {
            return Err(Error::Deadline);
        }
        Ok(bytes)
    };
    let result = match timed(clock, deadline, read).await {
        Ok(result) => result,
        Err(error) => Err(error),
    };
    let body = match result {
        Ok(body) => body,
        Err(error) => return Err(error.with_head(head)),
    };
    if clock.monotonic_now() >= deadline {
        return Err(Error::Deadline.with_head(head));
    }
    guard.complete = true;
    Ok(Reply {
        status: head.status,
        headers: head.headers,
        body,
    })
}
