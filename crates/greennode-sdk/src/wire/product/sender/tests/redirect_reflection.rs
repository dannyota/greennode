use super::*;

#[tokio::test]
async fn key_reflected_in_redirect_is_refused() {
    let server = Server::start(vec![
        response("302 Found", &[("Location", "/next?copy=to%79-key")]),
        response("302 Found", &[("Location", "/next?copy=to%79-key")]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let mut req = request(&server.origin);
    req.credentials.push(CredentialHeader {
        name: http::header::AUTHORIZATION,
        value: Secret::new("toy-key"),
        bearer: true,
    });
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Key,
        &req,
        &sleeper,
        &mut || 0
    )
    .await;
    let failure = match result {
        Err(error) => error,
        Ok(_) => panic!("expected refused redirect"),
    };
    assert!(matches!(failure.error, Error::Redirect(_)));
    assert_eq!(server.requests(), 2);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET / "))
    );
    assert!(failure.credentials.reflects(b"toy-key"));
}

async fn check_reflected_redirect(location: &str) {
    let server = Server::start(vec![
        response("302 Found", &[("Location", location)]),
        response("302 Found", &[("Location", location)]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    let failure = match result {
        Err(error) => error,
        Ok(_) => panic!("expected refused redirect"),
    };
    assert!(matches!(failure.error, Error::Redirect(_)));
    assert_eq!(server.requests(), 2);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET / "))
    );
    assert!(!format!("{failure:?}").contains("toy-first"));
}
#[tokio::test]
async fn redirect_raw_query_with_bearer_is_refused() {
    check_reflected_redirect("/next?copy=toy-first").await;
}
#[tokio::test]
async fn redirect_encoded_query_with_bearer_is_refused() {
    check_reflected_redirect("/next?copy=to%79-first").await;
}
#[tokio::test]
async fn redirect_encoded_path_with_bearer_is_refused() {
    check_reflected_redirect("/to%79-first/next").await;
}

#[tokio::test]
async fn redirect_base64_query_with_bearer_is_refused() {
    let encoded = data_encoding::BASE64.encode(b"toy-first");
    check_reflected_redirect(&format!("/next?copy={encoded}")).await;
}

async fn send_with_short_bearer(
    server: &Server,
    bearer: &str,
    initial_path: &str,
    path_operands: &[&str],
) -> Result<super::super::Response, Failure> {
    let config = crate::Config::new(
        crate::ConfigOptions::default()
            .with_region("han-1".into())
            .with_static_token(Secret::new(bearer)),
    )
    .unwrap();
    let clients = client(server);
    let (ctx, clock, deadline) = ctx();
    super::super::run(
        Runtime {
            configured_credentials: None,
            client: &clients,
            ctx: &ctx,
            clock: &clock,
            deadline,
            sleeper: &Immediate::default(),
            jitter: &mut || 0,
        },
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(config.session().bearer_source()),
        &request(&format!("{}{initial_path}", server.origin)),
        RouteInputs {
            path: path_operands,
            ..RouteInputs::default()
        },
    )
    .await
}

#[tokio::test]
async fn request_path_operands_refuse_prefixed_short_base64_credentials() {
    for (bearer, encoding) in [
        ("a?bcd", data_encoding::BASE64_NOPAD),
        ("ab>de", data_encoding::BASE64URL_NOPAD),
    ] {
        let id = format!("group-{}", encoding.encode(bearer.as_bytes()));
        let server = Server::start(vec![response("200 OK", &[])]);
        let outcome =
            send_with_short_bearer(&server, bearer, &format!("/groups/{id}"), &[&id]).await;
        assert!(outcome.is_err());
        assert_eq!(server.requests(), 0);
    }
}

#[tokio::test]
async fn redirect_path_refuses_short_base64_credentials() {
    let bearer = "ab>de";
    let standard = data_encoding::BASE64_NOPAD.encode(bearer.as_bytes());
    let url_safe = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
    for location in [format!("/{standard}"), format!("/group-{url_safe}")] {
        let server = Server::start(vec![
            response("302 Found", &[("Location", &location)]),
            response("302 Found", &[("Location", &location)]),
        ]);
        let outcome = send_with_short_bearer(&server, bearer, "/start", &[]).await;
        assert!(matches!(
            outcome,
            Err(Failure {
                error: Error::Redirect(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 2);
        assert!(
            server
                .heads()
                .iter()
                .all(|head| head.starts_with(b"GET /start "))
        );
    }
}

#[tokio::test]
async fn redirect_query_keys_and_values_refuse_short_base64_credentials() {
    let bearer = "ab>de";
    let encoded = data_encoding::BASE64URL_NOPAD.encode(bearer.as_bytes());
    let encoded = encoded.replace('-', "%2D");
    for location in [
        format!("/next?key-{encoded}-rest=ok"),
        format!("/next?key=value-{encoded}-rest"),
    ] {
        let server = Server::start(vec![
            response("302 Found", &[("Location", &location)]),
            response("302 Found", &[("Location", &location)]),
        ]);
        let outcome = send_with_short_bearer(&server, bearer, "/start", &[]).await;
        assert!(matches!(
            outcome,
            Err(Failure {
                error: Error::Redirect(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 2);
        assert!(
            server
                .heads()
                .iter()
                .all(|head| head.starts_with(b"GET /start "))
        );
    }
}
