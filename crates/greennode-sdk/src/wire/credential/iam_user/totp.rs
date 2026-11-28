use crate::Secret;

pub(crate) fn normalized_seed(seed: &Secret) -> Secret {
    use zeroize::Zeroizing;
    let raw = seed.expose_secret().trim();
    let mut normalized = Zeroizing::new(raw.to_owned());
    if raw
        .get(..10)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("otpauth://"))
        && let Ok(url) = url::Url::parse(raw)
        && let Some((_, value)) = url.query_pairs().find(|(name, _)| name == "secret")
        && !value.is_empty()
    {
        *normalized = value.into_owned();
    }
    *normalized = normalized
        .replace([' ', '-'], "")
        .trim_end_matches('=')
        .to_uppercase()
        .replace(['\r', '\n'], "");
    Secret::new(std::mem::take(&mut *normalized))
}

pub(crate) fn code(seed: &Secret, unix_seconds: i64) -> Result<Secret, ()> {
    use hmac::{Hmac, KeyInit, Mac};
    use zeroize::Zeroizing;
    let normalized = normalized_seed(seed);
    let key = Zeroizing::new(
        data_encoding::BASE32_NOPAD
            .decode(normalized.expose_secret().as_bytes())
            .map_err(|_| ())?,
    );
    let step = unix_seconds / 30;
    let counter = u64::try_from(step).map_err(|_| ())?;
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).map_err(|_| ())?;
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = usize::from(hash[19] & 15);
    let number = u32::from_be_bytes([
        hash[offset],
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]) & 0x7fff_ffff;
    Ok(Secret::new(format!("{:06}", number % 1_000_000)))
}

#[derive(Debug)]
pub(crate) enum ProviderError {
    Failed,
    TokenCache,
}

pub(crate) type Provider<'a> = dyn Fn() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Secret, ProviderError>> + Send + 'a>,
    > + Send
    + Sync
    + 'a;

pub(crate) enum Source<'a> {
    Provider(&'a Provider<'a>),
    Seed(&'a Secret),
}

async fn provider_code(
    provider: &Provider<'_>,
    clock: &dyn crate::Clock,
    deadline: tokio::time::Instant,
) -> Result<Secret, ProviderError> {
    if clock.monotonic_now() >= deadline {
        return Err(ProviderError::Failed);
    }
    let mut timer = clock.sleep_until(deadline);
    let mut future = provider();
    std::future::poll_fn(|cx| {
        if clock.monotonic_now() >= deadline || timer.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(ProviderError::Failed));
        }
        future.as_mut().poll(cx)
    })
    .await
}

fn allowed_target(target: &url::Url, origin: &url::Url) -> bool {
    target.origin() == origin.origin()
        && super::is_two_factor_path(target.path())
        && target.username().is_empty()
        && target.password().is_none()
        && target.fragment().is_none()
}

pub(super) async fn complete(
    login: &super::signin::Login<'_>,
    jar: &mut super::CookieJar,
    location: &Secret,
    source: Option<Source<'_>>,
) -> Result<Secret, crate::LoginError> {
    use crate::LoginError;
    use crate::wire::send::{self, Encoding};
    use bytes::Bytes;
    use http::{Request, header};
    use http_body_util::Full;

    let source = source.ok_or_else(|| LoginError::new("totp requirement", None, false))?;
    let target = url::Url::parse(location.expose_secret())
        .map_err(|_| LoginError::new("totp page", None, false))?;
    if !allowed_target(&target, login.origin) {
        return Err(LoginError::new("totp page", None, false));
    }
    jar.sent.record(location);
    if let Some(query) = target.query() {
        for (_, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
            jar.sent.record(&Secret::new(value));
        }
    }
    for (_, value) in target.query_pairs() {
        jar.record_form(&Secret::new(value));
    }
    let mut request = Request::get(target.as_str())
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(header::ACCEPT_ENCODING, "gzip");
    if let Some(cookie) = jar.header(&target, login.clock) {
        request = request.header(header::COOKIE, cookie.expose_secret());
    }
    let request = request
        .body(Full::new(Bytes::new()))
        .map_err(|_| LoginError::new("totp page", None, false))?;
    let page = send::with_sent_credentials(
        &jar.sent,
        send::once(
            login.client.as_client(),
            request,
            1 << 20,
            Encoding::Gzip,
            login.clock,
            login.deadline,
        ),
    )
    .await
    .map_err(|error| {
        LoginError::new(
            "totp page",
            error.observed().map(|head| head.status.as_u16()),
            false,
        )
    })?;
    if page.status != http::StatusCode::OK {
        return Err(LoginError::new(
            "totp page",
            Some(page.status.as_u16()),
            false,
        ));
    }
    for value in page.headers.get_all(header::SET_COOKIE) {
        if let Ok(value) = value.to_str() {
            jar.store(&target, value, login.clock);
        }
    }
    let csrf = super::signin::extract_csrf(&String::from_utf8_lossy(&page.body))
        .ok_or_else(|| LoginError::new("totp page", Some(page.status.as_u16()), false))?;
    let code = match source {
        Source::Seed(seed) => {
            code(seed, login.clock.utc_now().unix_timestamp()).map_err(|_| ProviderError::Failed)
        }
        Source::Provider(provider) => provider_code(provider, login.clock, login.deadline).await,
    }
    .map_err(|error| match error {
        ProviderError::Failed => LoginError::new("totp code", None, false),
        ProviderError::TokenCache => LoginError::new("token cache", None, false)
            .with_kind(crate::error::LoginFailureKind::TokenCache),
    })?;
    jar.record_form(&csrf);
    jar.record_form(&code);
    let form = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("_csrf", csrf.expose_secret())
            .append_pair("token", code.expose_secret());
        Secret::new(form.finish())
    };
    let mut request = Request::post(target.as_str())
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, login.origin.origin().ascii_serialization())
        .header(header::REFERER, target.as_str())
        .header(header::ACCEPT_ENCODING, "gzip");
    if let Some(cookie) = jar.header(&target, login.clock) {
        request = request.header(header::COOKIE, cookie.expose_secret());
    }
    let request = request
        .body(Full::new(Bytes::copy_from_slice(
            form.expose_secret().as_bytes(),
        )))
        .map_err(|_| LoginError::new("totp submission", None, false))?;
    let mut evidence = send::Evidence::default();
    let reply = send::with_sent_credentials(
        &jar.sent,
        send::once_with_expected(
            login.client.as_client(),
            request,
            1 << 20,
            Encoding::Gzip,
            login.clock,
            login.deadline,
            (&mut evidence, &[]),
        ),
    )
    .await
    .map_err(|error| {
        LoginError::new(
            "totp submission",
            error.observed().map(|head| head.status.as_u16()),
            false,
        )
    })?;
    if reply.status == http::StatusCode::OK {
        return Err(
            LoginError::new("totp rejection", Some(reply.status.as_u16()), false)
                .with_kind(crate::error::LoginFailureKind::TotpRejected),
        );
    }
    let location = reply
        .headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok());
    match super::signin::classify(login.origin, login.dashboard, reply.status, location)
        .map_err(|_| LoginError::new("authorization code", Some(reply.status.as_u16()), false))?
    {
        super::signin::Next::Code(code) => Ok(code),
        super::signin::Next::TwoFactor(_) => Err(LoginError::new(
            "authorization code",
            Some(reply.status.as_u16()),
            false,
        )),
    }
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

    fn rfc_seed() -> Secret {
        Secret::new(data_encoding::BASE32_NOPAD.encode(b"12345678901234567890"))
    }

    fn response(status: &str, location: Option<&str>, body: &str) -> Response {
        let mut headers = vec![("Content-Length".into(), body.len().to_string())];
        if let Some(location) = location {
            headers.push(("Location".into(), location.into()));
        }
        Response::Http {
            status: status.into(),
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    async fn run(server: &Server, supplied: bool) -> Result<Secret, crate::LoginError> {
        run_with(server, if supplied { 1 } else { 0 }, None).await
    }

    async fn run_with(
        server: &Server,
        mode: u8,
        location: Option<&str>,
    ) -> Result<Secret, crate::LoginError> {
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
        let password = Secret::new("password-canary");
        let seed = rfc_seed();
        let login = super::super::signin::Login {
            client: &client,
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root",
            username: "user",
            password: &password,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(60),
        };
        let mut jar = super::super::CookieJar::new(origin.clone());
        let target = Secret::new(
            location
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{}/ap/auth/iam/google", server.origin)),
        );
        let invalid = Secret::new("invalid-seed-canary!");
        let provider = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, ProviderError>> + Send>> { Box::pin(async { Ok(Secret::new("totp-code-canary")) }) };
        let failed = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, ProviderError>> + Send>> { Box::pin(async { Err(ProviderError::Failed) }) };
        let source = match mode {
            1 => Some(Source::Seed(&seed)),
            2 => Some(Source::Seed(&invalid)),
            3 => Some(Source::Provider(&provider)),
            4 => Some(Source::Provider(&failed)),
            _ => None,
        };
        complete(&login, &mut jar, &target, source).await
    }

    #[tokio::test]
    async fn totp_fetch_and_submit() {
        let server = Server::start(vec![
            response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=abc"),
                "",
            ),
        ]);
        let result = run(&server, true).await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 2);
    }

    #[tokio::test]
    async fn totp_verify_subroute_is_accepted() {
        let server = Server::start(vec![
            response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=abc"),
                "",
            ),
        ]);
        let target = format!("{}/ap/auth/iam/google/verify", server.origin);
        assert!(run_with(&server, 1, Some(&target)).await.is_ok());
        assert_eq!(server.requests(), 2);
    }

    #[tokio::test]
    async fn totp_redirect_is_classified_without_body() {
        let (sent, _ready) = tokio::sync::oneshot::channel();
        let (eof, _closed) = tokio::sync::oneshot::channel();
        let stalled = Response::ObserveStall { head: b"HTTP/1.1 302 Found\r\nLocation: https://dashboard.test/return?code=abc\r\nContent-Length: 1\r\n\r\n".to_vec(), sent, eof };
        let truncated = Response::RawClose(b"HTTP/1.1 302 Found\r\nLocation: https://dashboard.test/return?code=abc\r\nContent-Length: 1\r\n\r\n".to_vec());
        for reply in [stalled, truncated] {
            let server = Server::start(vec![
                response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
                reply,
            ]);
            let result = tokio::time::timeout(Duration::from_secs(60), run(&server, true)).await;
            assert!(matches!(result, Ok(Ok(_))));
        }
    }

    #[tokio::test]
    async fn missing_totp_fails_before_page() {
        let server = Server::start(vec![response(
            "200 OK",
            None,
            "<input name=\"_csrf\" value=\"token\">",
        )]);
        let result = run(&server, false).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().reason(), "totp requirement");
        assert_eq!(server.requests(), 0);
    }

    #[tokio::test]
    async fn provider_deadline_wins_when_both_are_ready() {
        struct ReadyClock(tokio::time::Instant);
        impl Clock for ReadyClock {
            fn utc_now(&self) -> time::OffsetDateTime {
                time::OffsetDateTime::UNIX_EPOCH
            }
            fn monotonic_now(&self) -> tokio::time::Instant {
                self.0
            }
            fn sleep_until(
                &self,
                _: tokio::time::Instant,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
                Box::pin(std::future::ready(()))
            }
        }
        let clock = ReadyClock(tokio::time::Instant::now());
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let provider = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, ProviderError>> + Send + '_>> {
            Box::pin(async { calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok(Secret::new("toy")) })
        };
        assert!(
            provider_code(&provider, &clock, clock.0 + Duration::from_secs(1))
                .await
                .is_err()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        assert!(
            provider_code(
                &provider,
                &clock,
                clock.monotonic_now() + Duration::from_secs(1)
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn provider_code_is_accepted() {
        let server = Server::start(vec![
            response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=abc"),
                "",
            ),
        ]);
        let result = run_with(&server, 3, None).await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 2);
        assert!(!format!("{:?}", result.unwrap()).contains("totp-code-canary"));
    }

    #[test]
    fn target_must_match_origin() {
        let origin = url::Url::parse("https://signin.test/").unwrap();
        let good = url::Url::parse("https://signin.test/ap/auth/iam/google").unwrap();
        let bad = url::Url::parse("https://other.test/ap/auth/iam/google").unwrap();
        assert!(allowed_target(&good, &origin));
        assert!(!allowed_target(&bad, &origin));
    }

    #[tokio::test]
    async fn invalid_target_is_rejected_before_sending() {
        for bad in [
            "https://other.test/ap/auth/iam/google",
            "https://other.test/not-google",
        ] {
            let server = Server::start(vec![]);
            let result = run_with(&server, 1, Some(bad)).await;
            assert!(result.is_err());
            assert_eq!(result.err().unwrap().reason(), "totp page");
            assert_eq!(server.requests(), 0);
        }
    }

    #[tokio::test]
    async fn wrong_route_is_rejected_before_sending() {
        let server = Server::start(vec![
            response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=abc"),
                "",
            ),
        ]);
        let wrong = format!("{}/ap/auth/iam/other", server.origin);
        let result = run_with(&server, 1, Some(&wrong)).await;
        assert!(result.is_err());
        assert_eq!(server.requests(), 0);
    }

    #[tokio::test]
    async fn totp_page_failures_do_not_submit() {
        for bad in [
            Response::Close,
            response(
                "302 Found",
                Some("/elsewhere"),
                "<input name=\"_csrf\" value=\"token\">",
            ),
            response("500 Error", None, "<input name=\"_csrf\" value=\"token\">"),
            response("200 OK", None, "missing csrf"),
            response(
                "200 OK",
                None,
                &format!(
                    "<input name=\"_csrf\" value=\"token\">{}",
                    "x".repeat(1 << 20)
                ),
            ),
        ] {
            let server = Server::start(vec![
                bad,
                response(
                    "302 Found",
                    Some("https://dashboard.test/return?code=abc"),
                    "",
                ),
            ]);
            let result = run_with(&server, 1, None).await;
            assert!(result.is_err());
            assert_eq!(result.err().unwrap().reason(), "totp page");
            assert!(server.requests() <= 1);
        }
    }

    #[tokio::test]
    async fn code_failures_do_not_submit_or_leak() {
        for mode in [2, 4] {
            let server = Server::start(vec![
                response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
                response(
                    "302 Found",
                    Some("https://dashboard.test/return?code=abc"),
                    "",
                ),
            ]);
            let result = run_with(&server, mode, None).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), "totp code");
            assert!(!format!("{error:?}").contains("invalid-seed-canary"));
            assert_eq!(server.requests(), 1);
        }
    }

    #[tokio::test]
    async fn totp_submission_failures_are_safe() {
        for (bad, reason) in [
            (Response::Close, "totp submission"),
            (
                response("200 OK", None, "password-canary totp-code-canary"),
                "totp rejection",
            ),
            (response("302 Found", None, ""), "authorization code"),
            (
                response(
                    "302 Found",
                    Some("https://other.test/?code=totp-code-canary"),
                    "",
                ),
                "authorization code",
            ),
        ] {
            let server = Server::start(vec![
                response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
                bad,
            ]);
            let result = run_with(&server, 3, None).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), reason);
            assert!(!format!("{error:?}").contains("totp-code-canary"));
            assert!(!format!("{error:?}").contains("password-canary"));
        }
    }

    #[test]
    fn rfc_6238_sha1_vectors_six_digit() {
        let seed = rfc_seed();
        for (time, expected) in [
            (59, "287082"),
            (1111111109, "081804"),
            (1111111111, "050471"),
            (1234567890, "005924"),
            (2000000000, "279037"),
            (20000000000, "353130"),
        ] {
            let result = code(&seed, time);
            assert!(result.is_ok());
            assert!(result.unwrap().expose_secret() == expected);
        }
    }

    #[test]
    fn invalid_seed_and_negative_time_fail() {
        assert!(code(&Secret::new("!invalid!"), 59).is_err());
        assert!(code(&rfc_seed(), -30).is_err());
        assert!(code(&rfc_seed(), 59).is_ok());
    }

    #[test]
    fn empty_first_otpauth_secret_is_not_skipped() {
        let seed = rfc_seed();
        let input = Secret::new(format!(
            "otpauth://totp/account?secret=&secret={}",
            seed.expose_secret()
        ));
        let valid = Secret::new(format!(
            "otpauth://totp/account?secret={}&secret=",
            seed.expose_secret()
        ));
        assert!(code(&input, 59).is_err());
        assert!(code(&valid, 59).is_ok());
    }

    #[test]
    fn base32_ignores_internal_crlf() {
        let seed = rfc_seed();
        let raw = seed.expose_secret();
        let broken = Secret::new(format!("{}\r\n{}\n{}", &raw[..4], &raw[4..12], &raw[12..]));
        let expected = code(&seed, 59).unwrap();
        let result = code(&broken, 59);
        assert!(result.is_ok());
        assert!(result.unwrap().expose_secret() == expected.expose_secret());
    }

    #[test]
    fn otpauth_and_normalized_seed_match() {
        let raw = rfc_seed();
        let value = raw.expose_secret().to_ascii_lowercase();
        let seed = Secret::new(format!(
            "{} {}-{}====",
            &value[..4],
            &value[4..8],
            &value[8..]
        ));
        let uri = Secret::new(format!(
            "otpauth://totp/account?secret={}",
            raw.expose_secret()
        ));
        assert!(code(&seed, 59).is_ok());
        assert!(
            code(&seed, 59).unwrap().expose_secret() == code(&uri, 59).unwrap().expose_secret()
        );
    }
}
