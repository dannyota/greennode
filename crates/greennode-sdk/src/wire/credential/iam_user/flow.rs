use crate::{LoginError, Secret, Token};

pub(super) async fn run(
    login: super::signin::Login<'_>,
    token_url: &url::Url,
    source: Option<super::totp::Source<'_>>,
    pkce: (Secret, String),
) -> Result<Token, LoginError> {
    let super::signin::Session {
        verifier,
        next,
        mut jar,
    } = super::signin::signin(login, pkce).await?;
    let code = match next {
        super::signin::Next::Code(code) => code,
        super::signin::Next::TwoFactor(location) => {
            super::totp::complete(&login, &mut jar, &location, source).await?
        }
    };
    drop(jar);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        connection::{HeadLimit, OneSendClient, Trust},
        test_server::{Response, Server},
    };
    use crate::{Clock, SystemClock};
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
            response(
                "200 OK",
                None,
                r#"{"accessToken":"token-canary","expiresIn":3600}"#,
            ),
        ]);
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let token_url = origin.join("/token").unwrap();
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
        let provider = || -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Secret, ()>> + Send>> { Box::pin(async { Ok(Secret::new("totp-code-canary")) }) };
        let result = run(
            login,
            &token_url,
            Some(super::super::totp::Source::Provider(&provider)),
            super::super::from_bytes(&[2; 32]),
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 5);
        server.bodies()
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
        let verifier = super::super::from_bytes(&[2; 32]).0;
        assert!(actual.as_object().is_some_and(|map| map.len() == 5));
        assert!(actual["grantType"] == "authorization_code");
        assert!(actual["code"] == "auth-code-canary");
        assert!(actual["codeVerifier"] == verifier.expose_secret());
        assert!(actual["redirectUri"] == "https://dashboard.test/return");
        assert!(actual["scope"] == "openid");
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
            response(
                "200 OK",
                None,
                r#"{"accessToken":"token-canary","expiresIn":3600}"#,
            ),
        ]);
        let client =
            OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap();
        let origin = url::Url::parse(&server.origin).unwrap();
        let token_url = origin.join("/token").unwrap();
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
        let result = run(
            login,
            &token_url,
            Some(super::super::totp::Source::Seed(&seed)),
            super::super::from_bytes(&[2; 32]),
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(server.requests(), 5);
    }
}
