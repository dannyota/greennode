use super::*;

#[tokio::test]
async fn prior_bearer_remains_available_for_response_reflection() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        Response::Raw(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\ntoy-first".to_vec()),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
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
    .await
    .unwrap();
    assert_eq!(reply.status, 200);
    assert!(reply.credentials.reflects(&reply.body));
    assert!(
        reply
            .credentials
            .values()
            .iter()
            .any(|secret| secret.expose_secret() == "toy-next")
    );
    assert_eq!(reply.credentials.values().len(), 2);
}

#[tokio::test]
async fn typed_key_header_is_attached_and_recorded() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        Response::Raw(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\ntoy-key".to_vec()),
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
    let reply = run!(
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
    .await
    .unwrap();
    assert!(reply.credentials.reflects(&reply.body));
    assert_eq!(reply.credentials.values().len(), 1);
    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads.iter().all(|head| {
        head.windows(b"authorization: Bearer toy-key".len())
            .any(|v| v.eq_ignore_ascii_case(b"authorization: Bearer toy-key"))
    }));
}

#[tokio::test]
async fn plain_headers_reject_credentials() {
    for name in ["authorization", "x-api-key"] {
        let server = Server::start(vec![response("200 OK", &[])]);
        let clients = client(&server);
        let (ctx, clock, deadline) = ctx();
        let sleeper = Immediate::default();
        let mut req = request(&server.origin);
        req.headers.insert(
            http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            http::HeaderValue::from_static("toy-key"),
        );
        let result = run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &operation(Method::GET, redirect::Retry::Repeatable),
            Auth::None,
            &req,
            &sleeper,
            &mut || 0
        )
        .await;
        assert!(matches!(
            result,
            Err(Failure {
                error: Error::InvalidRequest,
                ..
            })
        ));
        assert_eq!(server.requests(), 0);
    }
}

#[tokio::test]
async fn plain_header_cannot_repeat_bearer_value() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let mut req = request(&server.origin);
    req.headers.insert(
        "x-note",
        http::HeaderValue::from_static("prefix-toy-first-suffix"),
    );
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &req,
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::InvalidRequest,
            ..
        })
    ));
    assert_eq!(server.requests(), 0);
}
#[tokio::test]
async fn initial_uri_with_bearer_is_refused() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let req = request(&format!("{}/?copy=to%79-first", server.origin));
    let result = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &req,
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::InvalidRequest,
            ..
        })
    ));
    assert_eq!(server.requests(), 0);
}
#[tokio::test]
async fn key_reflected_in_redirect_is_refused() {
    let server = Server::start(vec![
        response("302 Found", &[("Location", "/next?copy=to%79-key")]),
        response("200 OK", &[]),
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
    assert_eq!(server.requests(), 1);
    assert!(failure.credentials.reflects(b"toy-key"));
}

async fn check_reflected_redirect(location: &str) {
    let server = Server::start(vec![
        response("302 Found", &[("Location", location)]),
        response("200 OK", &[]),
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
    assert_eq!(server.requests(), 1);
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
