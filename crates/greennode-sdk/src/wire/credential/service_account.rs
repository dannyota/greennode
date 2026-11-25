use crate::wire::{
    connection::{HeadLimit, OneSendClient, Trust},
    send::{self, Encoding},
};
use crate::{Clock, RequestContext, Secret, ServiceAccountLoginError, Token};
use bytes::Bytes;
use http::{
    Request,
    header::{self, HeaderValue},
};
use http_body_util::Full;
use std::time::Duration;
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
) -> Result<Token, ServiceAccountLoginError> {
    exchange_at(id, secret, TOKEN_URL, Trust::webpki(), context, clock).await
}

fn decode(body: &[u8], start: OffsetDateTime, now: OffsetDateTime) -> Option<Token> {
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
    validate(body, &fields).ok()?;
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    let token = object.get("access_token")?.as_str()?;
    if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if let Some(kind) = object.get("token_type")
        && !kind.as_str()?.eq_ignore_ascii_case("bearer")
    {
        return None;
    }
    let seconds = object.get("expires_in")?.as_i64()?;
    if seconds <= 30 || seconds > i64::MAX / 1_000_000_000 {
        return None;
    }
    let expiry = start.checked_add(time::Duration::seconds(seconds))?;
    if expiry - now <= time::Duration::seconds(30) {
        return None;
    }
    Some(Token::new(Secret::new(token), Some(expiry)))
}

async fn exchange_at(
    id: &str,
    secret: &Secret,
    url: &str,
    trust: Trust,
    context: &RequestContext,
    clock: &dyn Clock,
) -> Result<Token, ServiceAccountLoginError> {
    let failed = |status| ServiceAccountLoginError::new("token request failed", status);
    if context.check(clock).is_some() {
        return Err(failed(None));
    }
    let request = request(id, secret, url).ok_or_else(|| failed(None))?;
    let client = OneSendClient::new(HeadLimit::Api, trust).map_err(|_| failed(None))?;
    let deadline = clock
        .monotonic_now()
        .checked_add(Duration::from_secs(120))
        .ok_or_else(|| failed(None))?;
    let bounded = context.child_with_deadline(deadline);
    let start = clock.utc_now();
    let mut evidence = send::Evidence::default();
    let result = {
        let send = send::once_with_evidence(
            client.as_client(),
            request,
            64 * 1024,
            Encoding::Gzip,
            clock,
            bounded.deadline().ok_or_else(|| failed(None))?,
            &mut evidence,
        );
        tokio::select! {
            biased;
            _ = context.wait(clock) => None,
            result = send => Some(result),
        }
    };
    let Some(result) = result else {
        return Err(failed(evidence.status.map(|status| status.as_u16())));
    };
    let status = match &result {
        Ok(reply) => Some(reply.status.as_u16()),
        Err(error) => error.observed().map(|head| head.status.as_u16()),
    };
    if context.check(clock).is_some() {
        return Err(failed(status));
    }
    if let Some(status) = status {
        if (300..400).contains(&status) {
            return Err(ServiceAccountLoginError::new(
                "token endpoint redirected request",
                Some(status),
            ));
        }
        if status != 200 {
            return Err(ServiceAccountLoginError::new(
                "token endpoint rejected request",
                Some(status),
            ));
        }
    }
    let reply = result.map_err(|error| {
        if matches!(error.kind(), send::Error::Deadline) {
            failed(status)
        } else if error.observed().is_some() {
            ServiceAccountLoginError::new("invalid token response", status)
        } else {
            failed(None)
        }
    })?;
    let token = decode(&reply.body, start, clock.utc_now())
        .ok_or_else(|| ServiceAccountLoginError::new("invalid token response", status))?;
    if context.check(clock).is_some() {
        return Err(failed(status));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        connection::Trust,
        test_server::{Response, Server},
    };
    use crate::{RequestContext, TestClock};

    async fn attempt(
        status: &str,
        body: &[u8],
        headers: Vec<(String, String)>,
    ) -> (Server, Result<Token, crate::ServiceAccountLoginError>) {
        let mut headers = headers;
        headers.push(("Content-Length".into(), body.len().to_string()));
        let server = Server::start(vec![Response::Http {
            status: status.into(),
            headers,
            body: body.to_vec(),
        }]);
        let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
        let (context, _) = RequestContext::new();
        let result = exchange_at(
            "client",
            &Secret::new("canary-secret"),
            &format!("{}/accounts-api/v2/auth/token", server.origin),
            Trust::test(server.root_der.clone()),
            &context,
            &clock,
        )
        .await;
        (server, result)
    }

    #[tokio::test]
    async fn unauthorized() {
        let (server, result) = attempt("401 Unauthorized", b"canary-secret", vec![]).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(error.reason(), "token endpoint rejected request");
        assert_eq!(error.status(), Some(401));
        let error_safe = !format!("{error:?}").contains("canary-secret");
        assert!(error_safe);
        let wrapped = crate::Error::new(crate::ErrorClass::Auth).with_service_login(error);
        let error_safe = !format!("{wrapped:?} {wrapped}").contains("canary-secret");
        assert!(error_safe);
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    #[tokio::test]
    async fn server_error() {
        let (server, result) = attempt("503 Unavailable", b"canary-secret", vec![]).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(error.reason(), "token endpoint rejected request");
        assert_eq!(error.status(), Some(503));
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    #[tokio::test]
    async fn redirect() {
        let location = "/redirect-canary-secret";
        let (server, result) =
            attempt("302 Found", b"", vec![("Location".into(), location.into())]).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(error.reason(), "token endpoint redirected request");
        assert_eq!(error.status(), Some(302));
        assert!(!format!("{error:?} {}", error.reason()).contains(location));
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    #[tokio::test]
    async fn valid_exchange() {
        let (server, result) = attempt(
            "200 OK",
            br#"{"access_token":"toy-token","expires_in":1800,"token_type":"bEaReR"}"#,
            vec![],
        )
        .await;
        let token = result.unwrap();
        assert_eq!(
            token.expires_at,
            Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1800))
        );
        let token_matches = token.access_token.expose_secret() == "toy-token";
        assert!(token_matches);
        let token_safe = !format!("{token:?}").contains("toy-token");
        assert!(token_safe);
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
        let bodies = server.bodies();
        let body_matches =
            bodies.len() == 1 && bodies[0] == br#"{"grant_type":"client_credentials"}"#;
        assert!(body_matches);
        let heads = server.heads();
        let Ok(raw_head) = String::from_utf8(heads[0].clone()) else {
            panic!("invalid request head");
        };
        let authorization_matches = raw_head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .any(|(name, value)| {
                name.eq_ignore_ascii_case("authorization")
                    && value.trim() == "Basic Y2xpZW50OmNhbmFyeS1zZWNyZXQ="
            });
        assert!(authorization_matches);
        let head = raw_head.to_ascii_lowercase();
        assert!(head.starts_with("post /accounts-api/v2/auth/token http/1.1\r\n"));
        assert!(head.contains("content-type: application/json\r\n"));
        assert!(head.contains("accept: application/json\r\n"));
        assert!(head.contains("accept-encoding: gzip\r\n"));
        assert!(!head.contains("cookie:"));
    }

    #[tokio::test]
    async fn gzip_response() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(br#"{"access_token":"toy-token","expires_in":31}"#)
            .unwrap();
        let encoded = encoder.finish().unwrap();
        let (server, result) = attempt(
            "200 OK",
            &encoded,
            vec![("Content-Encoding".into(), "gzip".into())],
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    #[tokio::test]
    async fn oversized_rejection_keeps_status() {
        let body = vec![b'x'; 65537];
        let (server, result) = attempt("401 Unauthorized", &body, vec![]).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(error.reason(), "token endpoint rejected request");
        assert_eq!(error.status(), Some(401));
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    #[tokio::test]
    async fn oversized_response() {
        let valid = br#"{"access_token":"toy","expires_in":1800}"#;
        let mut body = valid.to_vec();
        body.resize(65537, b' ');
        let (server, result) = attempt("200 OK", &body, vec![]).await;
        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(error.reason(), "invalid token response");
        assert_eq!(error.status(), Some(200));
        assert_eq!(server.requests(), 1);
        assert_eq!(server.heads().len(), 1);
    }

    macro_rules! invalid {
        ($name:ident, $body:expr) => {
            #[tokio::test]
            async fn $name() {
                let (server, result) = attempt("200 OK", $body.as_bytes(), vec![]).await;
                assert!(result.is_err());
                let error = result.err().unwrap();
                assert_eq!(error.reason(), "invalid token response");
                assert_eq!(error.status(), Some(200));
                let error_safe = !format!("{error:?} {}", error.reason()).contains("canary-secret");
                assert!(error_safe);
                assert_eq!(server.requests(), 1);
                assert_eq!(server.heads().len(), 1);
            }
        };
    }

    invalid!(null, "null");
    invalid!(array, "[]");
    invalid!(missing, "{}");
    invalid!(short_expiry, r#"{"access_token":"toy","expires_in":30}"#);
    invalid!(zero_expiry, r#"{"access_token":"toy","expires_in":0}"#);
    invalid!(negative_expiry, r#"{"access_token":"toy","expires_in":-1}"#);
    invalid!(
        fractional_expiry,
        r#"{"access_token":"toy","expires_in":31.0}"#
    );
    invalid!(
        string_expiry,
        r#"{"access_token":"toy","expires_in":"1800"}"#
    );
    invalid!(null_expiry, r#"{"access_token":"toy","expires_in":null}"#);
    invalid!(null_token, r#"{"access_token":null,"expires_in":1800}"#);
    invalid!(number_token, r#"{"access_token":1,"expires_in":1800}"#);
    invalid!(empty_token, r#"{"access_token":"","expires_in":1800}"#);
    invalid!(space_token, r#"{"access_token":"a b","expires_in":1800}"#);
    invalid!(
        control_token,
        "{\"access_token\":\"a\\u0000b\",\"expires_in\":1800}"
    );
    invalid!(
        null_type,
        r#"{"access_token":"toy","expires_in":1800,"token_type":null}"#
    );
    invalid!(
        bad_type,
        r#"{"access_token":"toy","expires_in":1800,"token_type":"MAC"}"#
    );
    invalid!(
        number_type,
        r#"{"access_token":"toy","expires_in":1800,"token_type":3}"#
    );
    invalid!(trailing, r#"{"access_token":"toy","expires_in":1800} {}"#);
    invalid!(
        duplicate_token,
        r#"{"access_token":"toy","access_token":"other","expires_in":1800}"#
    );
    invalid!(
        duplicate_expiry,
        r#"{"access_token":"toy","expires_in":1800,"expires_in":1801}"#
    );
    invalid!(
        duplicate_type,
        r#"{"access_token":"toy","expires_in":1800,"token_type":"bearer","token_type":"Bearer"}"#
    );
    invalid!(
        duration_overflow,
        r#"{"access_token":"toy","expires_in":9223372037}"#
    );
    invalid!(
        integer_overflow,
        r#"{"access_token":"toy","expires_in":9223372036854775808}"#
    );
    invalid!(truncated, r#"{"access_token":"toy","expires_in":1800"#);
}
