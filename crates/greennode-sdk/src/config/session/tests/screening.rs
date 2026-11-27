use super::*;

#[tokio::test]
async fn authenticate_withholds_reflected_service_login_reason_and_status() {
    use crate::wire::test_server::Server;
    for (credential, expected_status, expected_reason) in [
        ("401", None, "token endpoint rejected request"),
        ("rejected", Some(401), ""),
    ] {
        let login = Server::start(vec![response("401 Unauthorized", None, "{}")]);
        let mut config = Config::new(options().with_service_account(ServiceAccountAuth::new(
            "client-canary".into(),
            Secret::new(credential),
        )))
        .unwrap();
        let session = Arc::get_mut(&mut config.session).unwrap();
        let Auth::ServiceAccount(source) = &mut session.auth else {
            panic!("wrong auth kind");
        };
        source.set_test_endpoint(login.origin.clone(), login.root_der.clone());
        let result = config
            .authenticate(&RequestContext::new().0)
            .await
            .unwrap_err();
        let login_error = result.service_account_login_error().unwrap();
        assert!(login_error.reason() == expected_reason);
        assert_eq!(login_error.status(), expected_status);
        assert_eq!(login.requests(), 1);
    }
}

#[tokio::test]
async fn authenticate_withholds_status_or_reason_reflected_by_sent_cookie_or_csrf() {
    use crate::wire::test_server::{Response, Server};
    for (cookie, csrf, status, reason) in [
        ("200", "toy-csrf", None, "signin rejection"),
        ("toy-session", "rejection", Some(200), ""),
    ] {
        let page = format!(r#"<input name="_csrf" value="{csrf}">"#);
        let signin = Server::start(vec![
            Response::Http {
                status: "200 OK".into(),
                headers: vec![
                    ("Content-Length".into(), page.len().to_string()),
                    (
                        "Set-Cookie".into(),
                        format!("session={cookie}; Path=/; Secure"),
                    ),
                ],
                body: page.into_bytes(),
            },
            response("200 OK", None, "rejected"),
        ]);
        let mut config = Config::new(options().with_iam_user(iam()).with_endpoint_overrides(
            EndpointOverrides {
                signin: Some(signin.origin.clone()),
                dashboard: Some("https://dashboard.test/return".into()),
                ..Default::default()
            },
        ))
        .unwrap();
        let session = Arc::get_mut(&mut config.session).unwrap();
        let Auth::IamUser(source) = &mut session.auth else {
            panic!("wrong auth kind");
        };
        source.set_test_roots(vec![signin.root_der.clone()]);
        let error = config
            .authenticate(&RequestContext::new().0)
            .await
            .unwrap_err();
        let login = error.login_error().unwrap();
        assert_eq!(signin.requests(), 2);
        assert_eq!(login.status(), status);
        assert!(login.reason() == reason);
    }
}

#[tokio::test]
async fn token_exchange_failure_retains_cookie_evidence_from_signin() {
    use crate::wire::test_server::{Response, Server};
    let page = r#"<input name="_csrf" value="toy-csrf">"#;
    let signin = Server::start(vec![
        Response::Http {
            status: "200 OK".into(),
            headers: vec![
                ("Content-Length".into(), page.len().to_string()),
                (
                    "Set-Cookie".into(),
                    "session=exchange; Path=/; Secure".into(),
                ),
            ],
            body: page.as_bytes().to_vec(),
        },
        response(
            "302 Found",
            Some("https://dashboard.test/return/?code=toy-code"),
            "",
        ),
    ]);
    let token = Server::start(vec![response("401 Unauthorized", None, "{}")]);
    let mut config = Config::new(options().with_iam_user(iam()).with_endpoint_overrides(
        EndpointOverrides {
            signin: Some(signin.origin.clone()),
            dashboard: Some("https://dashboard.test/return".into()),
            token: Some(format!("{}/token", token.origin)),
            ..Default::default()
        },
    ))
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::IamUser(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_roots(vec![signin.root_der.clone(), token.root_der.clone()]);
    let error = config
        .authenticate(&RequestContext::new().0)
        .await
        .unwrap_err();
    let login = error.login_error().unwrap();
    assert!(login.reason().is_empty());
    assert_eq!(login.status(), Some(401));
    assert_eq!(signin.requests(), 2);
    assert_eq!(token.requests(), 1);
}
