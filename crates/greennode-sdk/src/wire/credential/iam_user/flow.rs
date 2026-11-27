pub(crate) use super::token::TimedToken;
use crate::{Clock, LoginError, Secret};

pub(crate) struct Login<'a> {
    pub(crate) origin: &'a url::Url,
    pub(crate) dashboard: &'a url::Url,
    pub(crate) root_email: &'a str,
    pub(crate) username: &'a str,
    pub(crate) password: &'a Secret,
    pub(crate) totp: Option<super::totp::Source<'a>>,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) deadline: tokio::time::Instant,
}

pub(crate) async fn run(login: Login<'_>, token_url: &url::Url) -> Result<TimedToken, LoginError> {
    run_with_trust(login, token_url, crate::wire::connection::Trust::webpki()).await
}

#[cfg(test)]
pub(crate) async fn run_test(
    login: Login<'_>,
    token_url: &url::Url,
    roots: Vec<rustls::pki_types::CertificateDer<'static>>,
) -> Result<TimedToken, LoginError> {
    run_with_trust(
        login,
        token_url,
        crate::wire::connection::Trust::test_roots(roots),
    )
    .await
}

async fn run_with_trust(
    login: Login<'_>,
    token_url: &url::Url,
    trust: crate::wire::connection::Trust,
) -> Result<TimedToken, LoginError> {
    use crate::wire::connection::{HeadLimit, OneSendClient};
    let client = OneSendClient::new(HeadLimit::Api, trust)
        .map_err(|_| LoginError::new("signin page", None, false))?;
    let source = login.totp;
    let login = super::signin::Login {
        client: &client,
        origin: login.origin,
        dashboard: login.dashboard,
        root_email: login.root_email,
        username: login.username,
        password: login.password,
        clock: login.clock,
        deadline: login.deadline,
    };
    run_with_rng(login, token_url, source, &mut |bytes| {
        getrandom::fill(bytes).map_err(|_| ())
    })
    .await
}

async fn run_with_rng(
    login: super::signin::Login<'_>,
    token_url: &url::Url,
    source: Option<super::totp::Source<'_>>,
    rng: &mut (dyn FnMut(&mut [u8]) -> Result<(), ()> + Send),
) -> Result<TimedToken, LoginError> {
    let pkce = super::pkce(rng).map_err(|_| LoginError::new("signin page", None, false))?;
    let mut jar = super::CookieJar::new(login.origin.clone());
    let result = async {
        let (verifier, next) = super::signin::signin_with_evidence(login, pkce, &mut jar).await?;
        let code = match next {
            super::signin::Next::Code(code) => code,
            super::signin::Next::TwoFactor(location) => {
                super::totp::complete(&login, &mut jar, &location, source).await?
            }
        };
        jar.record_form(&code);
        jar.record_form(&verifier);
        super::token::exchange(super::token::Exchange {
            client: login.client,
            token_url,
            dashboard: login.dashboard,
            code: &code,
            verifier: &verifier,
            clock: login.clock,
            deadline: login.deadline,
        })
        .await
    }
    .await;
    match result {
        Ok(mut token) => {
            token.credentials.extend(&jar.sent);
            Ok(token)
        }
        Err(mut error) => {
            error.screen(|text| jar.sent.text_reflects(text));
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        connection::{HeadLimit, OneSendClient, Trust},
        test_server::{Response, Server},
    };
    use crate::{Clock, Secret, SystemClock};
    use std::time::Duration;

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

    #[tokio::test]
    async fn entropy_failure_stops_before_any_send() {
        let server = Server::start(vec![response(
            "200 OK",
            None,
            "<input name=\"_csrf\" value=\"token\">",
        )]);
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let token_url = origin.join("/token").unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
        let password = Secret::new("toy");
        let login = super::super::signin::Login {
            client: &client,
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root",
            username: "user",
            password: &password,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        };
        let mut rng = |_: &mut [u8]| Err(());
        let result = run_with_rng(login, &token_url, None, &mut rng).await;
        assert!(result.is_err());
        assert_eq!(server.requests(), 0);
    }

    async fn captured_bodies() -> Vec<Vec<u8>> {
        let server = Server::start(vec![
            response(
                "200 OK",
                None,
                "<input name=\"_csrf\" value=\"login-csrf\">",
            ),
            response("302 Found", Some("/ap/auth/iam/google"), ""),
            response("200 OK", None, "<input name=\"_csrf\" value=\"totp-csrf\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=auth-code-canary"),
                "",
            ),
        ]);
        let token_server = Server::start(vec![response(
            "200 OK",
            None,
            r#"{"accessToken":"token-canary","expiresIn":3600}"#,
        )]);
        let client = OneSendClient::new(
            HeadLimit::Api,
            Trust::test_roots([server.root_der.clone(), token_server.root_der.clone()]),
        )
        .unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", token_server.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
        let password = Secret::new("password-canary");
        let login = super::super::signin::Login {
            client: &client,
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root@example.test",
            username: "username-test",
            password: &password,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        };
        let provider = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, super::super::totp::ProviderError>> + Send>> { Box::pin(async { Ok(Secret::new("totp-code-canary")) }) };
        let result = run_with_rng(
            login,
            &token_url,
            Some(super::super::totp::Source::Provider(&provider)),
            &mut |bytes| {
                bytes.fill(2);
                Ok(())
            },
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 4);
        assert_eq!(token_server.requests(), 1);
        let mut bodies = server.bodies();
        bodies.extend(token_server.bodies());
        bodies
    }

    #[tokio::test]
    async fn sends_password_form_on_wire() {
        let bodies = captured_bodies().await;
        let actual: Vec<_> = url::form_urlencoded::parse(&bodies[1]).collect();
        assert!(actual.len() == 4);
        for (key, expected) in [
            ("_csrf", "login-csrf"),
            ("rootEmail", "root@example.test"),
            ("username", "username-test"),
            ("password", "password-canary"),
        ] {
            assert!(
                actual
                    .iter()
                    .any(|(name, value)| name == key && value == expected)
            );
        }
    }

    #[tokio::test]
    async fn sends_totp_form_on_wire() {
        let bodies = captured_bodies().await;
        let actual: Vec<_> = url::form_urlencoded::parse(&bodies[3]).collect();
        assert!(actual.len() == 2);
        for (key, expected) in [("_csrf", "totp-csrf"), ("token", "totp-code-canary")] {
            assert!(
                actual
                    .iter()
                    .any(|(name, value)| name == key && value == expected)
            );
        }
    }

    #[tokio::test]
    async fn sends_token_exchange_on_wire() {
        let bodies = captured_bodies().await;
        let actual: serde_json::Value = serde_json::from_slice(&bodies[4]).unwrap();
        let verifier = data_encoding::BASE64URL_NOPAD.encode(&[2; 32]);
        assert!(actual.as_object().is_some_and(|map| map.len() == 5));
        assert!(actual["grantType"] == "authorization_code");
        assert!(actual["code"] == "auth-code-canary");
        assert!(actual["codeVerifier"] == verifier);
        assert!(actual["redirectUri"] == "https://dashboard.test/return");
        assert!(actual["scope"] == "openid");
    }

    #[tokio::test]
    async fn separate_peers_receive_expected_requests() {
        let page = "<input name=\"_csrf\" value=\"token\">";
        let signin = Server::start(vec![
            Response::Http {
                status: "200 OK".into(),
                headers: vec![
                    ("Content-Length".into(), page.len().to_string()),
                    ("Set-Cookie".into(), "session=toy; Path=/; Secure".into()),
                ],
                body: page.as_bytes().to_vec(),
            },
            response("302 Found", Some("/ap/auth/iam/google"), ""),
            response("200 OK", None, "<input name=\"_csrf\" value=\"token\">"),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=toy"),
                "",
            ),
        ]);
        let token = Server::start(vec![response(
            "200 OK",
            None,
            r#"{"accessToken":"toy","expiresIn":3600}"#,
        )]);
        let client = OneSendClient::new(
            HeadLimit::Api,
            Trust::test_roots([signin.root_der.clone(), token.root_der.clone()]),
        )
        .unwrap();
        let origin = url::Url::parse(&signin.origin).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", token.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
        let password = Secret::new("toy-password");
        let login = super::super::signin::Login {
            client: &client,
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root",
            username: "user",
            password: &password,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        };
        let provider = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, super::super::totp::ProviderError>> + Send>> { Box::pin(async { Ok(Secret::new("toy-code")) }) };
        let result = run_with_rng(
            login,
            &token_url,
            Some(super::super::totp::Source::Provider(&provider)),
            &mut |bytes| {
                bytes.fill(2);
                Ok(())
            },
        )
        .await;
        let credentials = result.unwrap().credentials;
        for fragment in ["toy-code", "toy-password", "token"] {
            assert!(credentials.reflects(fragment.as_bytes()));
        }
        assert_eq!(signin.requests(), 4);
        assert_eq!(token.requests(), 1);
        fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
            head.lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        }
        let heads = signin.heads();
        let lines: Vec<_> = heads.iter().map(|h| String::from_utf8_lossy(h)).collect();
        assert!(lines[0].starts_with("GET /ap/auth/iam/login?"));
        assert!(lines[1].starts_with("POST /ap/auth/iam/login?"));
        assert!(lines[2].starts_with("GET /ap/auth/iam/google "));
        assert!(lines[3].starts_with("POST /ap/auth/iam/google "));
        let totp_ref = format!("{}/ap/auth/iam/google", signin.origin);
        let first_path = lines[0]
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let password_ref = format!("{}{}", signin.origin, first_path);
        assert!(header(&lines[1], "origin") == Some(signin.origin.as_str()));
        assert!(header(&lines[1], "referer") == Some(password_ref.as_str()));
        assert!(header(&lines[3], "origin") == Some(signin.origin.as_str()));
        assert!(header(&lines[3], "referer") == Some(totp_ref.as_str()));
        for head in [&lines[1], &lines[2], &lines[3]] {
            assert!(header(head, "cookie") == Some("session=toy"));
        }
        let token_heads = token.heads();
        let token_head = String::from_utf8_lossy(&token_heads[0]);
        let basic = format!(
            "Basic {}",
            data_encoding::BASE64.encode(b"c9e78411-f2a2-41ba-a9e4-3c56263c181a:")
        );
        assert!(token_head.starts_with("POST /token "));
        assert!(header(&token_head, "authorization") == Some(basic.as_str()));
        assert!(header(&token_head, "cookie").is_none());
        let first_path = String::from_utf8_lossy(&heads[0]);
        let get_url = url::Url::parse(&format!(
            "{}{}",
            signin.origin,
            first_path
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
        ))
        .unwrap();
        let challenge = get_url
            .query_pairs()
            .find(|(name, _)| name == "codeChallenge")
            .unwrap()
            .1
            .into_owned();
        let token_bodies = token.bodies();
        let token_json: serde_json::Value = serde_json::from_slice(&token_bodies[0]).unwrap();
        let verifier = token_json["codeVerifier"].as_str().unwrap();
        use sha2::{Digest, Sha256};
        assert!(
            challenge
                == data_encoding::BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()))
        );
    }

    #[tokio::test]
    async fn dropped_iam_sends_keep_attempt_credentials() {
        use crate::wire::product::exit::recorded_exactly;
        for step in 0..5 {
            let (sent, received) = tokio::sync::oneshot::channel();
            let page = br#"<input name="_csrf" value="login-csrf">"#;
            let mut replies = vec![
                Response::Http {
                    status: "200 OK".into(),
                    headers: vec![
                        ("Content-Length".into(), page.len().to_string()),
                        (
                            "Set-Cookie".into(),
                            "session=toy-cookie; Path=/; Secure".into(),
                        ),
                    ],
                    body: page.to_vec(),
                },
                response(
                    "302 Found",
                    Some("/ap/auth/iam/google?session=totp-ticket"),
                    "",
                ),
                response("200 OK", None, r#"<input name="_csrf" value="totp-csrf">"#),
                response(
                    "302 Found",
                    Some("https://dashboard.test/return?code=authorization-code"),
                    "",
                ),
            ];
            replies.truncate(step);
            replies.push(Response::StallBeforeHead(sent));
            let server = Server::start(replies);
            let client =
                OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
            let origin = url::Url::parse(&server.origin).unwrap();
            let token_url = origin.join("/token").unwrap();
            let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
            let password = Secret::new("toy-password");
            let login = super::super::signin::Login {
                client: &client,
                origin: &origin,
                dashboard: &dashboard,
                root_email: "toy-root@example.test",
                username: "toy-user",
                password: &password,
                clock: &SystemClock,
                deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
            };
            let provider: &super::super::totp::Provider<'_> =
                &|| Box::pin(async { Ok(Secret::new("345678")) });
            let mut rng = |bytes: &mut [u8]| {
                bytes.fill(2);
                Ok(())
            };
            let (verifier, challenge) = super::super::from_bytes(&[2; 32]);
            let client_id = super::super::CLIENT_ID;
            let payload = data_encoding::BASE64.encode(format!("{client_id}:").as_bytes());
            let authorization = format!("Basic {payload}");
            let expected: [&[&str]; 5] = [
                &[client_id, "toy-root@example.test", &challenge],
                &["toy-cookie", "login-csrf", "toy-user", "toy-password"],
                &["totp-ticket"],
                &["totp-csrf", "345678"],
                &[
                    "authorization-code",
                    verifier.expose_secret(),
                    &payload,
                    &authorization,
                ],
            ];
            crate::RequestContext::with_response_screening(async {
                tokio::select! {
                    signal = received => signal.unwrap(),
                    _ = run_with_rng(login, &token_url, Some(super::super::totp::Source::Provider(provider)), &mut rng) => panic!("send should still be pending at step {step}"),
                }
                for (index, credential) in expected[..=step].iter().flat_map(|values| values.iter()).enumerate() {
                    assert!(recorded_exactly(credential), "missing evidence at step {step}, entry {index}");
                }
                for credential in expected[step + 1..].iter().flat_map(|values| values.iter()) {
                    assert!(!recorded_exactly(credential), "recorded evidence before step {step} completed");
                }
            }).await;
            assert_eq!(server.requests(), step + 1);
        }
    }

    #[tokio::test]
    async fn full_flow_with_totp_uses_one_send_per_step() {
        let page = "<input name=\"_csrf\" value=\"csrf-token\">";
        let server = Server::start(vec![
            response("200 OK", None, page),
            response("302 Found", Some("/ap/auth/iam/google"), ""),
            response("200 OK", None, page),
            response(
                "302 Found",
                Some("https://dashboard.test/return?code=auth-code-canary"),
                "",
            ),
        ]);
        let token_server = Server::start(vec![response(
            "200 OK",
            None,
            r#"{"accessToken":"token-canary","expiresIn":3600}"#,
        )]);
        let client = OneSendClient::new(
            HeadLimit::Api,
            Trust::test_roots([server.root_der.clone(), token_server.root_der.clone()]),
        )
        .unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let token_url = url::Url::parse(&format!("{}/token", token_server.origin)).unwrap();
        let dashboard = url::Url::parse("https://dashboard.test/return").unwrap();
        let password = Secret::new("password-canary");
        let seed = Secret::new(data_encoding::BASE32_NOPAD.encode(b"12345678901234567890"));
        let login = super::super::signin::Login {
            client: &client,
            origin: &origin,
            dashboard: &dashboard,
            root_email: "root",
            username: "user",
            password: &password,
            clock: &SystemClock,
            deadline: SystemClock.monotonic_now() + Duration::from_secs(5),
        };
        let result = run_with_rng(
            login,
            &token_url,
            Some(super::super::totp::Source::Seed(&seed)),
            &mut |bytes| {
                bytes.fill(2);
                Ok(())
            },
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 4);
        assert_eq!(token_server.requests(), 1);
    }
}
