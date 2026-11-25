use super::connection::Client;
use crate::Clock;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Full};
use std::{
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
}
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

pub(crate) async fn once(
    client: &Client,
    request: Request<Full<Bytes>>,
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
    let is_head = request.method() == Method::HEAD;
    let exchange = async {
        let response = client
            .request(request)
            .await
            .map_err(|_| Error::Transport)?;
        if response.status() == StatusCode::SWITCHING_PROTOCOLS {
            return Err(Error::SwitchingProtocols);
        }
        match encoding {
            Encoding::Identity => {
                if response
                    .headers()
                    .get_all(header::CONTENT_ENCODING)
                    .iter()
                    .any(|value| {
                        value.to_str().map_or(true, |s| {
                            s.split(',')
                                .any(|token| !token.trim().eq_ignore_ascii_case("identity"))
                        })
                    })
                {
                    return Err(Error::Encoding);
                }
            }
        }
        if !is_head
            && !response.status().is_informational()
            && response.status() != StatusCode::NO_CONTENT
            && response.status() != StatusCode::NOT_MODIFIED
        {
            let too_large = response
                .headers()
                .get_all(header::CONTENT_LENGTH)
                .iter()
                .any(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .is_some_and(|length| length > u64::try_from(cap).unwrap_or(u64::MAX))
                });
            if too_large {
                return Err(Error::BodyTooLarge);
            }
        }
        let (parts, mut body) = response.into_parts();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| Error::Body)?;
            if let Ok(data) = frame.into_data() {
                if data.get(cap.saturating_sub(bytes.len())).is_some() {
                    return Err(Error::BodyTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        Ok(Reply {
            status: parts.status,
            headers: parts.headers,
            body: bytes,
        })
    };
    let mut exchange = Box::pin(exchange);
    let mut timer = clock.sleep_until(deadline);
    poll_fn(|cx| {
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Error::Deadline));
        }
        exchange.as_mut().poll(cx)
    })
    .await
}
