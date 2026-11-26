use crate::wire::{
    connection::{HeadLimit, OneSendClient, Trust},
    send::{self, Encoding},
};
use crate::{Clock, Error, ErrorClass, RequestContext, Secret, ServiceAccountLoginError, Token};
use bytes::Bytes;
use http::{
    Request,
    header::{self, HeaderValue},
};
use http_body_util::Full;
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use time::OffsetDateTime;
use zeroize::Zeroizing;

const TOKEN_URL: &str = "https://iamapis.vngcloud.vn/accounts-api/v2/auth/token";
const BODY: &str = r#"{"grant_type":"client_credentials"}"#;

fn authorization(
    id: &str,
    secret: &Secret,
) -> Result<HeaderValue, http::header::InvalidHeaderValue> {
    let mut input = Zeroizing::new(Vec::with_capacity(
        id.len() + 1 + secret.expose_secret().len(),
    ));
    input.extend_from_slice(id.as_bytes());
    input.push(b':');
    input.extend_from_slice(secret.expose_secret().as_bytes());
    let mut output = Zeroizing::new(String::from("Basic "));
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        output.push(char::from(ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from((first & 3) << 4 | second >> 4)],
        ));
        output.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from((second & 15) << 2 | third >> 6)])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(third & 63)])
        } else {
            '='
        });
    }
    let mut value = HeaderValue::from_str(&output)?;
    value.set_sensitive(true);
    Ok(value)
}

fn request(id: &str, secret: &Secret, url: &str) -> Option<Request<Full<Bytes>>> {
    Request::post(url)
        .header(header::AUTHORIZATION, authorization(id, secret).ok()?)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json")
        .header(header::ACCEPT_ENCODING, "gzip")
        .header(header::USER_AGENT, "Go-http-client/1.1")
        .header(header::CONTENT_LENGTH, BODY.len())
        .body(Full::new(Bytes::from_static(BODY.as_bytes())))
        .ok()
}

pub(crate) async fn exchange(
    id: &str,
    secret: &Secret,
    context: &RequestContext,
    clock: &dyn Clock,
    timeout: Duration,
) -> Result<Token, Box<Error>> {
    exchange_at(
        id,
        secret,
        TOKEN_URL,
        Trust::webpki(),
        context,
        clock,
        timeout,
    )
    .await
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default, deserialize_with = "present_string")]
    token_type: Option<String>,
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn decode(body: &[u8], start: OffsetDateTime, elapsed: Duration) -> Option<Token> {
    use crate::wire::strict_json::{Field, validate};
    let fields = [
        Field {
            name: "access_token",
            children: &[],
        },
        Field {
            name: "token_type",
            children: &[],
        },
        Field {
            name: "expires_in",
            children: &[],
        },
    ];
    validate(body, &fields, 64 * 1024).ok()?;
    if body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return None;
    }
    let response: TokenResponse = serde_json::from_slice(body).ok()?;
    let token = response.access_token;
    if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if let Some(kind) = response.token_type
        && !kind.eq_ignore_ascii_case("bearer")
    {
        return None;
    }
    let seconds = response.expires_in;
    if seconds <= 30 || seconds > i64::MAX / 1_000_000_000 {
        return None;
    }
    let expiry = start.checked_add(time::Duration::seconds(seconds))?;
    if Duration::from_secs(u64::try_from(seconds).ok()?).saturating_sub(elapsed)
        <= Duration::from_secs(30)
    {
        return None;
    }
    Some(Token::new(Secret::new(token), Some(expiry)))
}

fn decode_at_clock(
    body: &[u8],
    start: OffsetDateTime,
    started: tokio::time::Instant,
    clock: &dyn Clock,
) -> Option<Token> {
    decode(
        body,
        start,
        clock.monotonic_now().saturating_duration_since(started),
    )
}

async fn exchange_at(
    id: &str,
    secret: &Secret,
    url: &str,
    trust: Trust,
    context: &RequestContext,
    clock: &dyn Clock,
    timeout: Duration,
) -> Result<Token, Box<Error>> {
    let failure = |reason, status| {
        let error = context
            .check(clock)
            .map_or_else(|| Error::new(ErrorClass::Auth), Error::from);
        Box::new(error.with_service_login(ServiceAccountLoginError::new(reason, status)))
    };
    let failed = |status| failure("token request failed", status);
    if context.check(clock).is_some() {
        return Err(failed(None));
    }
    let request = request(id, secret, url).ok_or_else(|| failed(None))?;
    let client = OneSendClient::new(HeadLimit::Api, trust).map_err(|_| failed(None))?;
    let deadline = clock
        .monotonic_now()
        .checked_add(if timeout.is_zero() {
            Duration::from_secs(120)
        } else {
            timeout
        })
        .ok_or_else(|| failed(None))?;
    let bounded = context.child_with_deadline(deadline);
    let start = clock.utc_now();
    let started = clock.monotonic_now();
    let mut evidence = send::Evidence::default();
    let result = {
        let send = send::once_with_expected(
            client.as_client(),
            request,
            64 * 1024,
            Encoding::Gzip,
            clock,
            bounded.deadline().ok_or_else(|| failed(None))?,
            (&mut evidence, &[http::StatusCode::OK]),
        );
        let mut send = Box::pin(send);
        let mut ended = Box::pin(context.wait(clock));
        poll_fn(|cx| {
            if ended.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            send.as_mut().poll(cx).map(Some)
        })
        .await
    };
    let Some(result) = result else {
        let status = evidence.status.map(|status| status.as_u16());
        return Err(if status == Some(200) {
            failure("invalid token response", status)
        } else {
            failed(status)
        });
    };
    let status = match &result {
        Ok(reply) => Some(reply.status.as_u16()),
        Err(error) => error.observed().map(|head| head.status.as_u16()),
    };
    if context.check(clock).is_some() {
        return Err(if status == Some(200) && result.is_err() {
            failure("invalid token response", status)
        } else {
            failed(status)
        });
    }
    if let Some(status) = status {
        if (300..400).contains(&status) {
            return Err(failure("token endpoint redirected request", Some(status)));
        }
        if status != 200 {
            return Err(failure("token endpoint rejected request", Some(status)));
        }
    }
    let reply = result.map_err(|error| {
        if error.observed().is_some() {
            failure("invalid token response", status)
        } else {
            failed(None)
        }
    })?;
    let token = decode_at_clock(&reply.body, start, started, clock)
        .ok_or_else(|| failure("invalid token response", status))?;
    if context.check(clock).is_some() {
        return Err(failed(status));
    }
    Ok(token)
}

#[cfg(test)]
mod tests;
