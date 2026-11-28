use super::{
    connection::{Client, DialFailure},
    product::retry::{Outcome, SendFailure},
};
use crate::Clock;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper::body::Body;
use hyper_util::client::legacy::connect::{CaptureConnection, capture_connection};
use std::{
    fmt,
    future::{Future, poll_fn},
    io::{self, Write},
    task::Poll,
};
use tokio::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Encoding {
    Identity,
    Gzip,
}

struct Bounded {
    bytes: Vec<u8>,
    cap: usize,
    overflow: bool,
}

impl Write for Bounded {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data
            .get(self.cap.saturating_sub(self.bytes.len()))
            .is_some()
        {
            self.overflow = true;
            return Err(io::Error::other("body too large"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidRequest,
    BodyTooLarge,
    Deadline,
    SwitchingProtocols,
    Encoding,
    Transport(SendFailure, NetworkCause),
    Body,
    BodyNetwork(NetworkCause),
    AfterHead { reason: Box<Error>, head: Observed },
}

#[derive(PartialEq, Eq)]
pub(crate) struct Observed {
    pub(crate) status: StatusCode,
}

impl fmt::Debug for Observed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observed")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Error {
    pub(crate) fn retry_outcome(&self) -> Option<Outcome<'static>> {
        match self {
            Self::Transport(failure, _) => Some(Outcome::Send(*failure)),
            Self::AfterHead { .. } => Some(Outcome::Body),
            _ => None,
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkCause {
    Other,
    Dial,
    Read,
    Write,
    ConnectionRefused,
    ConnectionReset,
    DialConnectionReset,
    WriteConnectionReset,
    NoSuchHost,
    TimedOut,
    CertificateUnknownAuthority,
    CertificateHostnameMismatch,
    CertificateInvalid,
}

impl NetworkCause {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Other => "network request failed",
            Self::Dial => "dial: network request failed",
            Self::Read => "read: network request failed",
            Self::Write => "write: network request failed",
            Self::ConnectionRefused => "dial: connect: connection refused",
            Self::ConnectionReset => "read: connection reset",
            Self::DialConnectionReset => "dial: connection reset",
            Self::WriteConnectionReset => "write: connection reset",
            Self::NoSuchHost => "dial: no such host",
            Self::TimedOut => "timed out",
            Self::CertificateUnknownAuthority => {
                "TLS certificate verification failed: unknown authority"
            }
            Self::CertificateHostnameMismatch => {
                "TLS certificate verification failed: hostname mismatch"
            }
            Self::CertificateInvalid => "TLS certificate verification failed: invalid certificate",
        }
    }
}

fn error_chain<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> impl Iterator<Item = &'a (dyn std::error::Error + 'static)> + Clone {
    std::iter::successors(error.source(), |cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &(dyn std::error::Error + 'static))
            .or_else(|| cause.source())
    })
}

fn body_frame_failure(error: hyper::Error) -> Error {
    error_chain(&error)
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .find_map(|io| io_failure_cause(io, false))
        .map_or(Error::Body, Error::BodyNetwork)
}

fn io_failure_cause(error: &io::Error, dial: bool) -> Option<NetworkCause> {
    use super::connection::IoPhase;
    let phase = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<IoPhase>());
    match error.kind() {
        io::ErrorKind::TimedOut => Some(NetworkCause::TimedOut),
        io::ErrorKind::ConnectionRefused if dial => Some(NetworkCause::ConnectionRefused),
        io::ErrorKind::ConnectionReset if dial => Some(NetworkCause::DialConnectionReset),
        io::ErrorKind::ConnectionReset if phase == Some(&IoPhase::Write) => {
            Some(NetworkCause::WriteConnectionReset)
        }
        io::ErrorKind::ConnectionReset => Some(NetworkCause::ConnectionReset),
        _ if dial => None,
        _ => phase.map(|phase| match phase {
            IoPhase::Read => NetworkCause::Read,
            IoPhase::Write => NetworkCause::Write,
        }),
    }
}

pub(super) fn transport_failure(error: &hyper_util::client::legacy::Error) -> Error {
    let causes = error_chain(error);
    let dial = error.is_connect() && causes.clone().any(|cause| cause.is::<DialFailure>());
    let failure = if dial {
        SendFailure::FailedDial
    } else {
        SendFailure::PossiblySent
    };
    let detail = causes.clone().find_map(|cause| {
        if let Some(rustls::Error::InvalidCertificate(reason)) =
            cause.downcast_ref::<rustls::Error>()
        {
            return Some(match reason {
                rustls::CertificateError::UnknownIssuer => {
                    NetworkCause::CertificateUnknownAuthority
                }
                rustls::CertificateError::NotValidForName
                | rustls::CertificateError::NotValidForNameContext { .. } => {
                    NetworkCause::CertificateHostnameMismatch
                }
                _ => NetworkCause::CertificateInvalid,
            });
        }
        io_failure_cause(cause.downcast_ref::<io::Error>()?, dial)
    });
    let dns_not_found = || {
        causes
            .filter_map(|cause| cause.downcast_ref::<io::Error>())
            .any(|cause| {
                let text = cause.to_string();
                text.starts_with("failed to lookup address information: ")
                    && [
                        "Name or service not known",
                        "nodename nor servname provided, or not known",
                        "No address associated with hostname",
                    ]
                    .iter()
                    .any(|suffix| text.ends_with(suffix))
            })
    };
    let detail = detail.unwrap_or_else(|| {
        if !dial {
            NetworkCause::Other
        } else if dns_not_found() {
            NetworkCause::NoSuchHost
        } else {
            NetworkCause::Dial
        }
    });
    Error::Transport(failure, detail)
}

#[derive(Default)]
pub(crate) struct Evidence<'a> {
    #[cfg(test)]
    pub(crate) attempt_counter: Option<&'a dyn crate::wire::product::sender::Sleeper>,
    pub(crate) attempted: bool,
    #[cfg(not(test))]
    marker: std::marker::PhantomData<&'a ()>,
    pub(crate) started: bool,
    pub(crate) status: Option<StatusCode>,
}

pub(crate) async fn once(
    client: &Client,
    request: Request<Full<Bytes>>,
    cap: usize,
    encoding: Encoding,
    clock: &dyn Clock,
    deadline: Instant,
) -> Result<Reply, Error> {
    once_inner(
        client,
        request,
        cap,
        encoding,
        clock,
        deadline,
        (&mut Evidence::default(), |_, _| true),
    )
    .await
}

pub(crate) async fn once_with_evidence(
    client: &Client,
    request: Request<Full<Bytes>>,
    cap: usize,
    encoding: Encoding,
    clock: &dyn Clock,
    deadline: Instant,
    action: (
        &mut Evidence<'_>,
        impl FnOnce(StatusCode, &HeaderMap) -> bool + Send,
    ),
) -> Result<Reply, Error> {
    once_inner(client, request, cap, encoding, clock, deadline, action).await
}

pub(crate) async fn once_with_expected(
    client: &Client,
    request: Request<Full<Bytes>>,
    cap: usize,
    encoding: Encoding,
    clock: &dyn Clock,
    deadline: Instant,
    (evidence, expected): (&mut Evidence<'_>, &[StatusCode]),
) -> Result<Reply, Error> {
    once_inner(
        client,
        request,
        cap,
        encoding,
        clock,
        deadline,
        (evidence, |status, _| expected.contains(&status)),
    )
    .await
}

async fn once_inner(
    client: &Client,
    mut request: Request<Full<Bytes>>,
    cap: usize,
    encoding: Encoding,
    clock: &dyn Clock,
    deadline: Instant,
    (evidence, read_body): (
        &mut Evidence<'_>,
        impl FnOnce(StatusCode, &HeaderMap) -> bool + Send,
    ),
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
    let mut request = Box::pin(client.request(request));
    let send = poll_fn(|cx| {
        if !evidence.attempted {
            evidence.attempted = true;
            #[cfg(test)]
            if let Some(counter) = evidence.attempt_counter {
                counter.record_write_attempt();
            }
        }
        let result = request.as_mut().poll(cx);
        evidence.started |= guard.capture.connection_metadata().is_some();
        result
    });
    let response = timed(clock, deadline, send)
        .await?
        .map_err(|error| transport_failure(&error))?;
    let (parts, mut body) = response.into_parts();
    evidence.status = Some(parts.status);
    let head = Observed {
        status: parts.status,
    };
    let headers = parts.headers;
    if clock.monotonic_now() >= deadline {
        return Err(Error::Deadline.with_head(head));
    }
    if head.status == StatusCode::SWITCHING_PROTOCOLS {
        return Err(Error::SwitchingProtocols.with_head(head));
    }
    if !read_body(head.status, &headers) {
        drop(guard);
        drop(body);
        return Ok(Reply {
            status: head.status,
            headers,
            body: Vec::new(),
        });
    }
    let values = headers.get_all(header::CONTENT_ENCODING);
    let mut values = values.iter();
    let first = values.next();
    let second = values.next();
    let token = first.and_then(|v| v.to_str().ok()).map(str::trim);
    let gzip =
        matches!(encoding, Encoding::Gzip) && token.is_some_and(|s| s.eq_ignore_ascii_case("gzip"));
    if second.is_some()
        || (first.is_some() && !gzip && !token.is_some_and(|s| s.eq_ignore_ascii_case("identity")))
    {
        return Err(Error::Encoding.with_head(head));
    }
    let bodyless = is_head
        || head.status.is_informational()
        || head.status == StatusCode::NO_CONTENT
        || head.status == StatusCode::NOT_MODIFIED;
    let encoded_cap = if gzip { cap.saturating_add(65536) } else { cap };
    if !bodyless
        && body
            .size_hint()
            .exact()
            .is_some_and(|n| n > u64::try_from(encoded_cap).unwrap_or(u64::MAX))
    {
        return Err(Error::BodyTooLarge.with_head(head));
    }
    let read = async {
        let mut bytes = Vec::new();
        let mut encoded = 0usize;
        let mut decoder = (gzip && !bodyless).then(|| {
            flate2::write::MultiGzDecoder::new(Bounded {
                bytes: Vec::new(),
                cap,
                overflow: false,
            })
        });
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
            let frame = frame.map_err(body_frame_failure)?;
            if let Ok(data) = frame.into_data() {
                if data.get(encoded_cap.saturating_sub(encoded)).is_some() {
                    return Err(Error::BodyTooLarge);
                }
                encoded += data.len();
                if let Some(decoder) = decoder.as_mut() {
                    let mut remaining = data.as_ref();
                    while !remaining.is_empty() {
                        if clock.monotonic_now() >= deadline {
                            return Err(Error::Deadline);
                        }
                        let result = decoder.write(&remaining[..remaining.len().min(4096)]);
                        if clock.monotonic_now() >= deadline {
                            return Err(Error::Deadline);
                        }
                        match result {
                            Ok(0) => return Err(Error::Body),
                            Ok(n) => remaining = &remaining[n..],
                            Err(_) if decoder.get_ref().overflow => {
                                return Err(Error::BodyTooLarge);
                            }
                            Err(_) => return Err(Error::Body),
                        }
                    }
                } else {
                    bytes.extend_from_slice(&data);
                }
            }
        }
        if let Some(mut decoder) = decoder {
            if clock.monotonic_now() >= deadline {
                return Err(Error::Deadline);
            }
            let result = decoder.try_finish();
            if clock.monotonic_now() >= deadline {
                return Err(Error::Deadline);
            }
            if result.is_err() {
                return Err(if decoder.get_ref().overflow {
                    Error::BodyTooLarge
                } else {
                    Error::Body
                });
            }
            bytes = decoder.finish().map_err(|_| Error::Body)?.bytes;
        }
        if clock.monotonic_now() >= deadline {
            return Err(Error::Deadline);
        }
        Ok(bytes)
    };
    #[cfg(any(test, feature = "test-support"))]
    clock.response_body_started();
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
        headers,
        body,
    })
}

pub(crate) async fn with_sent_credentials<T>(
    credentials: &crate::wire::product::evidence::SentCredentials,
    send: impl std::future::Future<Output = T>,
) -> T {
    crate::wire::product::exit::observe_sent(credentials);
    send.await
}
