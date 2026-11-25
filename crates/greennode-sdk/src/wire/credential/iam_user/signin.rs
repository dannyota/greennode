use crate::{LoginError, Secret};
use url::Url;

#[derive(Debug)]
pub(super) enum Next {
    TwoFactor(Secret),
    Code(Secret),
}

fn classify(
    origin: &Url,
    dashboard: &Url,
    status: http::StatusCode,
    location: Option<&str>,
) -> Result<Next, LoginError> {
    let failure = |reason, captcha| LoginError::new(reason, Some(status.as_u16()), captcha);
    if status == http::StatusCode::MOVED_PERMANENTLY
        || status == http::StatusCode::PERMANENT_REDIRECT
    {
        return Err(failure("signin submission", false));
    }
    if status == http::StatusCode::OK {
        return Err(failure("signin rejection", true));
    }
    let location = location.ok_or_else(|| failure("signin submission", false))?;
    let target = origin
        .join(location)
        .map_err(|_| failure("authorization code", false))?;
    if !target.username().is_empty() || target.password().is_some() || target.fragment().is_some() {
        return Err(failure("authorization code", false));
    }
    if target.origin() == origin.origin() && target.path() == "/ap/auth/iam/google" {
        return Ok(Next::TwoFactor(Secret::new(target.as_str())));
    }
    if target.origin() != dashboard.origin() || target.path() != dashboard.path() {
        return Err(failure("authorization code", false));
    }
    let mut codes = target.query_pairs().filter(|(key, _)| key == "code");
    let Some((_, code)) = codes.next() else {
        return Err(failure("authorization code", false));
    };
    if code.is_empty() || codes.next().is_some() {
        return Err(failure("authorization code", false));
    }
    Ok(Next::Code(Secret::new(code.into_owned())))
}

pub(super) struct Login<'a> {
    pub(super) client: &'a crate::wire::connection::OneSendClient,
    pub(super) origin: &'a Url,
    pub(super) dashboard: &'a Url,
    pub(super) root_email: &'a str,
    pub(super) username: &'a str,
    pub(super) password: &'a Secret,
    pub(super) clock: &'a dyn crate::Clock,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) struct Session {
    pub(super) verifier: Secret,
    pub(super) next: Next,
    pub(super) jar: super::CookieJar,
}

pub(super) async fn signin(
    login: Login<'_>,
    pkce: (Secret, String),
) -> Result<Session, LoginError> {
    use crate::wire::send::{self, Encoding};
    use bytes::Bytes;
    use http::{Request, header};
    use http_body_util::Full;

    let page_error = || LoginError::new("signin page", None, false);
    if login.origin.scheme() != "https"
        || !login.origin.username().is_empty()
        || login.origin.password().is_some()
    {
        return Err(page_error());
    }
    let mut target = login
        .origin
        .join("/ap/auth/iam/login")
        .map_err(|_| page_error())?;
    target
        .query_pairs_mut()
        .append_pair("clientId", "c9e78411-f2a2-41ba-a9e4-3c56263c181a")
        .append_pair("responseType", "code")
        .append_pair("codeChallenge", &pkce.1)
        .append_pair("codeChallengeMethod", "S256")
        .append_pair("redirectUri", login.dashboard.as_str())
        .append_pair("rootEmail", login.root_email);
    let mut jar = super::CookieJar::new(login.origin.clone());
    let request = Request::get(target.as_str())
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Full::new(Bytes::new()))
        .map_err(|_| page_error())?;
    let page = send::once(
        login.client.as_client(),
        request,
        1 << 20,
        Encoding::Gzip,
        login.clock,
        login.deadline,
    )
    .await
    .map_err(|e| {
        LoginError::new(
            "signin page",
            e.observed().map(|h| h.status.as_u16()),
            false,
        )
    })?;
    if page.status != http::StatusCode::OK {
        return Err(LoginError::new(
            "signin page",
            Some(page.status.as_u16()),
            false,
        ));
    }
    for value in page.headers.get_all(header::SET_COOKIE) {
        if let Ok(value) = value.to_str() {
            jar.store(&target, value, login.clock);
        }
    }
    let text = String::from_utf8_lossy(&page.body);
    let csrf = extract_csrf(&text)
        .ok_or_else(|| LoginError::new("signin page", Some(page.status.as_u16()), false))?;
    let form = password_form(&csrf, login.root_email, login.username, login.password);
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
        .map_err(|_| LoginError::new("signin submission", None, false))?;
    let reply = send::once(
        login.client.as_client(),
        request,
        1 << 20,
        Encoding::Gzip,
        login.clock,
        login.deadline,
    )
    .await
    .map_err(|e| {
        LoginError::new(
            "signin submission",
            e.observed().map(|h| h.status.as_u16()),
            false,
        )
    })?;
    for value in reply.headers.get_all(header::SET_COOKIE) {
        if let Ok(value) = value.to_str() {
            jar.store(&target, value, login.clock);
        }
    }
    let location = reply
        .headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok());
    let next = classify(login.origin, login.dashboard, reply.status, location)?;
    Ok(Session {
        verifier: pkce.0,
        next,
        jar,
    })
}

fn password_form(csrf: &Secret, root: &str, user: &str, password: &Secret) -> Secret {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("_csrf", csrf.expose_secret())
        .append_pair("rootEmail", root)
        .append_pair("username", user)
        .append_pair("password", password.expose_secret());
    Secret::new(form.finish())
}

fn extract_csrf(text: &str) -> Option<Secret> {
    for pattern in [
        r#"content="([^"]+)"[^>]*name="csrf-token""#,
        r#"name="_csrf"[^>]*value="([^"]+)""#,
    ] {
        if let Ok(regex) = regex_lite::Regex::new(pattern)
            && let Some(value) = regex.captures(text).and_then(|c| c.get(1))
        {
            return Some(Secret::new(value.as_str()));
        }
    }
    None
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

    fn response(status: &str, headers: Vec<(&str, &str)>, body: &str) -> Response {
        let mut headers: Vec<_> = headers
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        headers.push(("Content-Length".into(), body.len().to_string()));
        Response::Http {
            status: status.into(),
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    async fn run(server: &Server) -> Result<Session, LoginError> {
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let origin = url(&server.origin);
        let dashboard = url("https://dashboard.test/return");
        let password = Secret::new("password-canary-value");
        signin(
            Login {
                client: &client,
                origin: &origin,
                dashboard: &dashboard,
                root_email: "root@example.test",
                username: "user",
                password: &password,
                clock: &SystemClock,
                deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
            },
            super::super::from_bytes(&[1; 32]),
        )
        .await
    }

    #[tokio::test]
    async fn page_and_password_callback() {
        let server = Server::start(vec![
            response(
                "200 OK",
                vec![("Set-Cookie", "session=toy; Path=/; Secure")],
                "<input name=\"_csrf\" value=\"token\">",
            ),
            response(
                "302 Found",
                vec![("Location", "https://dashboard.test/return?code=one")],
                "",
            ),
        ]);
        let result = run(&server).await;
        assert!(matches!(
            result,
            Ok(Session {
                next: Next::Code(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 2);
        let heads = server.heads();
        let get = String::from_utf8_lossy(&heads[0]);
        assert!(get.contains("codeChallengeMethod=S256"));
        let post = String::from_utf8_lossy(&heads[1]).to_ascii_lowercase();
        assert!(post.contains("cookie: session=toy"));
    }

    #[tokio::test]
    async fn moved_page_is_failure_without_following() {
        let server = Server::start(vec![
            response(
                "302 Found",
                vec![("Location", "/moved")],
                "<input name=\"_csrf\" value=\"token\">",
            ),
            response("200 OK", vec![], ""),
        ]);
        let result = run(&server).await;
        assert!(result.is_err());
        assert_eq!(server.requests(), 1);
    }

    #[tokio::test]
    async fn missing_csrf_fails_before_password() {
        let server = Server::start(vec![
            response("200 OK", vec![], "missing token"),
            response(
                "302 Found",
                vec![("Location", "https://dashboard.test/return?code=one")],
                "",
            ),
        ]);
        let result = run(&server).await;
        assert!(result.is_err());
        assert_eq!(server.requests(), 1);
    }

    #[tokio::test]
    async fn page_status_and_body_limit_fail_before_submission() {
        for page in [
            response(
                "500 Error",
                vec![],
                "<input name=\"_csrf\" value=\"token\">",
            ),
            response(
                "200 OK",
                vec![],
                &format!(
                    "<input name=\"_csrf\" value=\"token\">{}",
                    "x".repeat(1 << 20)
                ),
            ),
        ] {
            let server = Server::start(vec![
                page,
                response(
                    "302 Found",
                    vec![("Location", "https://dashboard.test/return?code=one")],
                    "",
                ),
            ]);
            let result = run(&server).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), "signin page");
            assert_eq!(server.requests(), 1);
        }
    }

    #[tokio::test]
    async fn password_rejections_have_safe_reasons() {
        for (status, location, reason, captcha) in [
            (
                "301 Moved Permanently",
                Some("https://outside.test/password-canary-value"),
                "signin submission",
                false,
            ),
            (
                "308 Permanent Redirect",
                Some("/other"),
                "signin submission",
                false,
            ),
            ("200 OK", None, "signin rejection", true),
            ("403 Forbidden", None, "signin submission", false),
        ] {
            let headers = location.map(|s| vec![("Location", s)]).unwrap_or_default();
            let server = Server::start(vec![
                response("200 OK", vec![], "<input name=\"_csrf\" value=\"token\">"),
                response(status, headers, "password-canary-value"),
            ]);
            let result = run(&server).await;
            assert!(result.is_err());
            let error = result.err().unwrap();
            assert_eq!(error.reason(), reason);
            assert_eq!(error.captcha_suspected(), captcha);
            assert!(!format!("{error:?}").contains("password-canary-value"));
            assert!(!error.reason().contains("password-canary-value"));
            assert_eq!(server.requests(), 2);
        }
    }

    #[tokio::test]
    async fn transport_failures_are_attributed_to_the_step() {
        let server = Server::start(vec![Response::Close]);
        let result = run(&server).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().reason(), "signin page");
        let server = Server::start(vec![
            response("200 OK", vec![], "<input name=\"_csrf\" value=\"token\">"),
            Response::Close,
        ]);
        let result = run(&server).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().reason(), "signin submission");
    }

    #[tokio::test]
    async fn password_response_body_is_bounded() {
        let server = Server::start(vec![
            response("200 OK", vec![], "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                vec![("Location", "https://dashboard.test/return?code=one")],
                &"x".repeat((1 << 20) + 1),
            ),
        ]);
        let result = run(&server).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().reason(), "signin submission");
    }

    #[tokio::test]
    async fn two_factor_location_is_classified_without_visiting() {
        let server = Server::start(vec![
            response(
                "200 OK",
                vec![],
                "<meta content=\"token\" name=\"csrf-token\">",
            ),
            response("302 Found", vec![("Location", "/ap/auth/iam/google")], ""),
        ]);
        let result = run(&server).await;
        assert!(matches!(
            result,
            Ok(Session {
                next: Next::TwoFactor(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 2);
    }

    #[test]
    fn password_form_contains_encoded_fields() {
        let password = Secret::new("password-canary-value");
        let form = password_form(
            &Secret::new("token"),
            "root@example.test",
            "user",
            &password,
        );
        let pairs: Vec<_> = url::form_urlencoded::parse(form.expose_secret().as_bytes()).collect();
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "_csrf" && value == "token")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "rootEmail" && value == "root@example.test")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "username" && value == "user")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "password" && value == password.expose_secret())
        );
        assert_eq!(pairs.len(), 4);
        assert!(!format!("{form:?}").contains("password-canary-value"));
    }

    #[test]
    fn csrf_extraction_matches_baseline_order() {
        let text =
            "<meta content=\"first\" name=\"csrf-token\"><input name=\"_csrf\" value=\"second\">";
        assert_eq!(extract_csrf(text).unwrap().expose_secret(), "first");
        assert!(extract_csrf("<input value=\"wrong\" name=\"_csrf\">").is_none());
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn unexpected_callback_is_rejected_on_loopback() {
        for location in [
            "https://other.test/return?code=abc",
            "https://dashboard.test:444/return?code=abc",
            "https://dashboard.test/other?code=abc",
            "http://dashboard.test/return?code=abc",
            "https://dashboard.test/return?code=abc&code=def",
            "https://dashboard.test/return?code=",
            "https://dashboard.test/return",
            "https://other.test/ap/auth/iam/google",
            "/ap/auth/iam/google/other",
        ] {
            let server = Server::start(vec![
                response("200 OK", vec![], "<input name=\"_csrf\" value=\"token\">"),
                response("302 Found", vec![("Location", location)], ""),
            ]);
            let result = run(&server).await;
            assert!(result.is_err());
            assert_eq!(server.requests(), 2);
        }
    }

    #[test]
    fn expected_callback_only() {
        let origin = url("https://signin.test/");
        let dashboard = url("https://dashboard.test/return");
        let classify_location = |s| classify(&origin, &dashboard, http::StatusCode::FOUND, Some(s));
        assert!(matches!(
            classify_location("https://dashboard.test/return?code=abc"),
            Ok(Next::Code(_))
        ));
        assert!(matches!(
            classify_location("https://dashboard.test/return?other=ignored&code=abc"),
            Ok(Next::Code(_))
        ));
        for bad in [
            "https://other.test/return?code=abc",
            "https://dashboard.test:444/return?code=abc",
            "https://dashboard.test/other?code=abc",
            "http://dashboard.test/return?code=abc",
            "https://dashboard.test/return?code=abc&code=def",
            "https://dashboard.test/return?code=",
            "https://dashboard.test/return",
        ] {
            assert!(classify_location(bad).is_err());
        }
    }

    #[test]
    fn two_factor_route_is_same_origin_and_exact() {
        let origin = url("https://signin.test/");
        let dashboard = url("https://dashboard.test/return");
        let classify_location = |s| classify(&origin, &dashboard, http::StatusCode::FOUND, Some(s));
        assert!(matches!(
            classify_location("/ap/auth/iam/google"),
            Ok(Next::TwoFactor(_))
        ));
        assert!(classify_location("https://other.test/ap/auth/iam/google").is_err());
        assert!(classify_location("/ap/auth/iam/google/other").is_err());
    }
}
