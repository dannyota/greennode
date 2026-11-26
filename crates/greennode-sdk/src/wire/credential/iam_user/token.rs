use crate::wire::strict_json::{Field, field_matches, validate};
use crate::{LoginError, Secret, Token};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

fn decode(
    bytes: &[u8],
    started: time::OffsetDateTime,
    elapsed: std::time::Duration,
) -> Result<Token, LoginError> {
    let err = || LoginError::new("token exchange", None, false);
    let fields = [
        Field {
            name: "accessToken",
            children: &[],
        },
        Field {
            name: "refreshToken",
            children: &[],
        },
        Field {
            name: "expiresIn",
            children: &[],
        },
    ];
    validate(bytes, &fields, 1024 * 1024).map_err(|_| err())?;
    let fields: BTreeMap<String, &RawValue> = serde_json::from_slice(bytes).map_err(|_| err())?;
    let mut access = None;
    let mut expires = None;
    for (name, value) in fields {
        if field_matches(&name, "accessToken") {
            let value: Option<String> = serde_json::from_str(value.get()).map_err(|_| err())?;
            access = Some(Secret::new(value.unwrap_or_default()));
        } else if field_matches(&name, "expiresIn") {
            let value: Option<i64> = serde_json::from_str(value.get()).map_err(|_| err())?;
            expires = Some(value.unwrap_or_default());
        } else if field_matches(&name, "refreshToken") {
            let _: Option<String> = serde_json::from_str(value.get()).map_err(|_| err())?;
        }
    }
    let access = access.ok_or_else(err)?;
    let expires = expires.ok_or_else(err)?;
    if access.expose_secret().is_empty() || expires <= 30 || expires > i64::MAX / 1_000_000_000 {
        return Err(err());
    }
    let end = started
        .checked_add(time::Duration::seconds(expires))
        .ok_or_else(err)?;
    if std::time::Duration::from_secs(u64::try_from(expires).map_err(|_| err())?)
        .saturating_sub(elapsed)
        <= std::time::Duration::from_secs(30)
    {
        return Err(err());
    }
    Ok(Token::new(access, Some(end)))
}

pub(super) struct Exchange<'a> {
    pub(super) client: &'a crate::wire::connection::OneSendClient,
    pub(super) token_url: &'a url::Url,
    pub(super) dashboard: &'a url::Url,
    pub(super) code: &'a Secret,
    pub(super) verifier: &'a Secret,
    pub(super) clock: &'a dyn crate::Clock,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) async fn exchange(input: Exchange<'_>) -> Result<Token, LoginError> {
    use crate::wire::send::{self, Encoding};
    use bytes::Bytes;
    use data_encoding::BASE64;
    use http::{Request, header};
    use http_body_util::Full;

    let failure = || LoginError::new("token exchange", None, false);
    if input.token_url.scheme() != "https"
        || !input.token_url.username().is_empty()
        || input.token_url.password().is_some()
        || input.token_url.fragment().is_some()
    {
        return Err(failure());
    }
    #[derive(serde::Serialize)]
    struct Body<'a> {
        #[serde(rename = "grantType")]
        grant_type: &'static str,
        code: &'a str,
        #[serde(rename = "redirectUri")]
        redirect_uri: &'a str,
        scope: &'static str,
        #[serde(rename = "codeVerifier")]
        verifier: &'a str,
    }
    let body = Secret::new(
        serde_json::to_string(&Body {
            grant_type: "authorization_code",
            code: input.code.expose_secret(),
            redirect_uri: input.dashboard.as_str(),
            scope: "openid",
            verifier: input.verifier.expose_secret(),
        })
        .map_err(|_| failure())?,
    );
    let authorization = format!(
        "Basic {}",
        BASE64.encode(b"c9e78411-f2a2-41ba-a9e4-3c56263c181a:")
    );
    let request = Request::post(input.token_url.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, authorization)
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Full::new(Bytes::copy_from_slice(
            body.expose_secret().as_bytes(),
        )))
        .map_err(|_| failure())?;
    let started = input.clock.utc_now();
    let started_monotonic = input.clock.monotonic_now();
    let response = send::once(
        input.client.as_client(),
        request,
        1 << 20,
        Encoding::Gzip,
        input.clock,
        input.deadline,
    )
    .await
    .map_err(|error| {
        LoginError::new(
            "token exchange",
            error.observed().map(|head| head.status.as_u16()),
            false,
        )
    })?;
    let status = Some(response.status.as_u16());
    if response.status != http::StatusCode::OK {
        return Err(LoginError::new("token exchange", status, false));
    }
    decode(
        &response.body,
        started,
        input
            .clock
            .monotonic_now()
            .saturating_duration_since(started_monotonic),
    )
    .map_err(|_| LoginError::new("token exchange", status, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::wire::{
        connection::{HeadLimit, OneSendClient, Trust},
        test_server::{Response, Server},
    };
    use crate::{Clock, SystemClock};
    use std::time::Duration;

    fn response(status: &str, body: &str) -> Response {
        Response::Http {
            status: status.into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    async fn run(server: &Server) -> Result<Token, LoginError> {
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", server.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/").unwrap();
        let (verifier, _) = super::super::pkce(&mut |bytes| {
            bytes.fill(3);
            Ok(())
        })
        .unwrap();
        exchange(Exchange {
            client: &client,
            token_url: &token_url,
            dashboard: &dashboard,
            code: &Secret::new("auth-code-canary"),
            verifier: &verifier,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        })
        .await
    }

    #[tokio::test]
    async fn token_exchange_succeeds_once() {
        let server = Server::start(vec![response(
            "200 OK",
            r#"{"accessToken":"token-canary","expiresIn":3600}"#,
        )]);
        let result = run(&server).await;
        assert!(result.is_ok());
        assert!(!format!("{:?}", result.unwrap()).contains("token-canary"));
        assert_eq!(server.requests(), 1);
        let head = String::from_utf8_lossy(&server.heads()[0]).to_ascii_lowercase();
        assert!(head.starts_with("post /token "));
        assert!(head.contains("content-type: application/json"));
    }

    #[tokio::test]
    async fn token_failures_do_not_leak_secrets() {
        for response in [
            Response::Close,
            response(
                "302 Found",
                r#"{"accessToken":"auth-code-canary","expiresIn":3600,"extra":"verifier-canary"}"#,
            ),
            response(
                "500 Error",
                r#"{"accessToken":"auth-code-canary","expiresIn":3600,"extra":"verifier-canary"}"#,
            ),
            response("200 OK", "auth-code-canary verifier-canary"),
            response(
                "200 OK",
                &format!(
                    r#"{{"accessToken":"abc","expiresIn":3600,"extra":"{}"}}"#,
                    "x".repeat(1 << 20)
                ),
            ),
        ] {
            let server = Server::start(vec![response]);
            let result = run(&server).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), "token exchange");
            assert!(!format!("{error:?}").contains("auth-code-canary"));
            assert!(!format!("{error:?}").contains("verifier-canary"));
        }
    }

    #[tokio::test]
    async fn elapsed_time_uses_monotonic_clock_even_if_wall_clock_moves_back() {
        struct ShiftingClock {
            base: tokio::time::Instant,
            ticks: std::sync::atomic::AtomicUsize,
            walls: std::sync::atomic::AtomicUsize,
        }
        impl Clock for ShiftingClock {
            fn utc_now(&self) -> time::OffsetDateTime {
                let call = self.walls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                time::OffsetDateTime::UNIX_EPOCH
                    + time::Duration::seconds(if call == 0 { 100 } else { 0 })
            }
            fn monotonic_now(&self) -> tokio::time::Instant {
                self.base
                    + Duration::from_secs(
                        2 * self.ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as u64,
                    )
            }
            fn sleep_until(
                &self,
                _: tokio::time::Instant,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(std::future::pending())
            }
        }
        let server = Server::start(vec![response(
            "200 OK",
            r#"{"accessToken":"toy","expiresIn":31}"#,
        )]);
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", server.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/").unwrap();
        let base = tokio::time::Instant::now();
        let clock = ShiftingClock {
            base,
            ticks: std::sync::atomic::AtomicUsize::new(0),
            walls: std::sync::atomic::AtomicUsize::new(0),
        };
        let (verifier, _) = super::super::pkce(&mut |bytes| {
            bytes.fill(3);
            Ok(())
        })
        .unwrap();
        let result = exchange(Exchange {
            client: &client,
            token_url: &token_url,
            dashboard: &dashboard,
            code: &Secret::new("toy-code"),
            verifier: &verifier,
            clock: &clock,
            deadline: base + Duration::from_secs(300),
        })
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn unicode_token_aliases_follow_baseline() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        let valid = "{\"accessToKen\":\"abc\",\"expiresIn\":3600}";
        let duplicate = "{\"accessToken\":\"abc\",\"accessToKen\":\"other\",\"expiresIn\":3600}";
        assert!(decode(valid.as_bytes(), now, Duration::ZERO).is_ok());
        assert!(decode(duplicate.as_bytes(), now, Duration::ZERO).is_err());
    }

    #[test]
    fn iam_token_schema_allows_one_mib() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        let valid = format!(
            r#"{{"accessToken":"abc","expiresIn":3600,"extra":"{}"}}"#,
            "a".repeat(70 * 1024)
        );
        let oversized = format!(
            r#"{{"accessToken":"abc","expiresIn":3600,"extra":"{}"}}"#,
            "a".repeat(1024 * 1024)
        );
        assert!(decode(valid.as_bytes(), now, Duration::ZERO).is_ok());
        assert!(decode(oversized.as_bytes(), now, Duration::ZERO).is_err());
    }

    #[test]
    fn token_schema_and_lifetime() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        assert!(
            decode(
                br#"{"accessToken":"abc","expiresIn":3600}"#,
                now,
                Duration::ZERO
            )
            .is_ok()
        );
        for bad in [
            br#"{"accessToken":"","expiresIn":3600}"#.as_slice(),
            br#"{"accessToken":"abc","expiresIn":30}"#,
            br#"{"accessToken":"abc","expiresIn":9223372037}"#,
            br#"{"accessToken":"abc","expiresIn":"3600"}"#,
            br#"{"accessToken":"abc","expiresIn":3600,"accessToken":"other"}"#,
            br#"{"accessToken":"abc","ACCESSTOKEN":"other","expiresIn":3600}"#,
            br#"{"accessToken":"abc","expiresIn":3600,"unknown":{"x":1,"x":2}}"#,
            br#"{"accessToken":"abc","expiresIn":3600} {}"#,
        ] {
            assert!(decode(bad, now, Duration::ZERO).is_err());
        }
        assert!(
            decode(
                br#"{"accessToken":"abc","expiresIn":30}"#,
                now,
                Duration::ZERO
            )
            .is_err()
        );
        assert!(
            decode(
                br#"{"accessToken":"abc","expiresIn":31}"#,
                now,
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
