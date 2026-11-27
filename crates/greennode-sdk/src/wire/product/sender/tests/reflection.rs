use super::*;

#[test]
fn identifier_query_uses_strict_credential_matching() {
    let credentials = SentCredentials::for_test(vec![Secret::new("abc")]);
    let inputs = RouteInputs {
        query: &[("alarm", "prefixYWJjsuffix")],
        identifier_query_keys: &["alarm"],
        ..Default::default()
    };
    assert!(inputs.reflects(&credentials));
    let ordinary = RouteInputs {
        query: &[("text", "prefixYWJjsuffix")],
        ..Default::default()
    };
    assert!(!ordinary.reflects(&credentials));
}

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
    let values = reply.credentials.values();
    assert_eq!(values.len(), 4);
    for expected in [
        "toy-first",
        "Bearer toy-first",
        "toy-next",
        "Bearer toy-next",
    ] {
        assert!(values.iter().any(|value| value.expose_secret() == expected));
    }
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
    let values = reply.credentials.values();
    assert_eq!(values.len(), 2);
    for expected in ["toy-key", "Bearer toy-key"] {
        assert!(values.iter().any(|value| value.expose_secret() == expected));
    }
    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads.iter().all(|head| {
        head.windows(b"authorization: Bearer toy-key".len())
            .any(|v| v.eq_ignore_ascii_case(b"authorization: Bearer toy-key"))
    }));
}

#[tokio::test]
async fn write_route_and_body_cannot_reflect_the_full_credential_header() {
    let encoded_header = format!(
        "pre{}post",
        data_encoding::BASE64URL_NOPAD.encode(b"Bearer jkl")
    );
    for in_route in [true, false] {
        let server = Server::start(vec![response("200 OK", &[])]);
        let clients = client(&server);
        let (ctx, clock, deadline) = ctx();
        let sleeper = Immediate::default();
        let mut req = request(&server.origin);
        req.credentials.push(CredentialHeader {
            name: http::header::AUTHORIZATION,
            value: Secret::new("jkl"),
            bearer: true,
        });
        let route = [encoded_header.as_str()];
        let mut inputs = RouteInputs::default();
        if in_route {
            req.url = req.url.join(&encoded_header).unwrap();
            inputs.path = &route;
        } else {
            req.body = Bytes::from(serde_json::json!({"value": &encoded_header}).to_string());
        }
        let write = Operation {
            class: redirect::Classification::Write,
            ..operation(Method::POST, redirect::Retry::Once)
        };
        let failure = super::super::run(
            Runtime {
                configured_credentials: None,
                client: &clients,
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &sleeper,
                jitter: &mut || 0,
            },
            &write,
            Auth::Key,
            &req,
            inputs,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(failure.error, Error::Provider(_)));
        assert_eq!(failure.dispatch, crate::WriteDispatch::NotSent);
        assert_eq!(server.requests(), 0);
    }
}

#[tokio::test]
async fn raw_uri_and_header_checks_refuse_uncompiled_credentials() {
    let credentials = SentCredentials::for_test(vec![Secret::new("x".repeat(64 << 10))]);
    let mut refused = [false; 2];
    for (index, raw_header) in [false, true].into_iter().enumerate() {
        let server = Server::start(vec![response("200 OK", &[])]);
        let (ctx, clock, deadline) = ctx();
        let mut req = request(&format!("{}/ordinary?path=ordinary", server.origin));
        let auth = if raw_header {
            req.headers.insert(
                "x-note",
                http::HeaderValue::from_bytes(b"\xffordinary").unwrap(),
            );
            Auth::None
        } else {
            req.credentials.push(CredentialHeader {
                name: http::header::AUTHORIZATION,
                value: credentials.values()[0].clone(),
                bearer: true,
            });
            Auth::Key
        };
        let result = super::super::run(
            Runtime {
                configured_credentials: Some(&credentials),
                client: &client(&server),
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &Immediate::default(),
                jitter: &mut || 0,
            },
            &operation(Method::GET, redirect::Retry::Once),
            auth,
            &req,
            RouteInputs::default(),
        )
        .await;
        refused[index] = result
            .err()
            .is_some_and(|failure| matches!(failure.error, Error::InvalidRequest))
            && server.requests() == 0;
    }
    assert_eq!(refused, [true, true]);
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
async fn encoded_header_cannot_repeat_refreshed_bearer() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        response("200 OK", &[]),
    ]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let mut req = request(&server.origin);
    req.headers
        .insert("x-note", http::HeaderValue::from_static("to%79-next"));
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
    assert_eq!(server.requests(), 1);
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

#[tokio::test]
async fn redaction_only_values_can_be_sent_but_credentials_cannot() {
    use crate::wire::product::evidence::Evidence;

    let server = Server::start(vec![response("200 OK", &[]), response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let mut evidence = Evidence::new();
    evidence.add_redaction(&Secret::new("toy-resource-label"));
    evidence.absorb(&SentCredentials::for_test(vec![Secret::new(
        "toy-configured",
    )]));
    let op = operation(Method::POST, redirect::Retry::Once);
    for (body, query, accepted) in [
        ("toy-resource-label", "toy-resource-label", true),
        ("toy%2dconfigured", "toy-resource-label", false),
        ("toy%2dfirst", "toy-resource-label", false),
        ("toy-resource-label", "toy%2dfirst", false),
    ] {
        let mut request = request(&server.origin);
        request.url.query_pairs_mut().append_pair("label", query);
        request.body = Bytes::from(serde_json::to_vec(&serde_json::json!({"label":body})).unwrap());
        let source = Provider::default();
        let outcome = super::super::run(
            Runtime {
                configured_credentials: Some(evidence.credentials()),
                client: &clients,
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &Immediate::default(),
                jitter: &mut || 0,
            },
            &op,
            Auth::Bearer(&PublicSource(&source)),
            &request,
            RouteInputs {
                path: &[],
                query: &[("label", query)],
                ..RouteInputs::default()
            },
        )
        .await;
        if accepted {
            let response = outcome.unwrap();
            assert!(response.credentials.reflects(b"toy-resource-label"));
        } else {
            assert!(outcome.is_err());
        }
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn database_password_slot_refuses_reused_configured_iam_password() {
    use crate::wire::product::evidence::Evidence;
    use crate::{Config, ConfigOptions, IamUserAuth};

    let password = Secret::new("ToyOperationPassword9");
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_iam_user(IamUserAuth::new(
                "toy@example.test".into(),
                "toy-user".into(),
                password.clone(),
            )),
    )
    .unwrap();
    let server = Server::start(vec![
        response("200 OK", &[]),
        response("200 OK", &[]),
        response("200 OK", &[]),
    ]);
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, clock, deadline) = ctx();
    let mut jitter = || 0;
    let configured = config
        .session()
        .runtime(&ctx, &mut jitter)
        .unwrap()
        .configured_credentials
        .unwrap();
    let clients = client(&server);
    let mut request = request(&server.origin);
    request.body = Bytes::from(
        serde_json::to_vec(
            &serde_json::json!({"user":{"password":password.expose_secret()},"name":"toy-db"}),
        )
        .unwrap(),
    );
    let operation = operation(Method::POST, redirect::Retry::Once);
    let mut refused = [false; 3];
    for (index, order) in [None, Some(false), Some(true)].into_iter().enumerate() {
        let mut evidence = Evidence::new();
        if order == Some(true) {
            evidence.absorb(configured);
        }
        evidence.add_operation_password(&password);
        if order == Some(false) {
            evidence.absorb(configured);
        }
        let source = Provider::default();
        let result = super::super::run(
            Runtime {
                configured_credentials: Some(evidence.credentials()),
                client: &clients,
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &Immediate::default(),
                jitter: &mut || 0,
            },
            &operation,
            Auth::Bearer(&PublicSource(&source)),
            &request,
            RouteInputs {
                password_slot: Some(crate::wire::product::options::PasswordSlot::DatabaseUser(
                    &password,
                )),
                ..RouteInputs::default()
            },
        )
        .await;
        refused[index] = match result {
            Ok(_) => false,
            Err(Failure {
                error: Error::Provider(error),
                ..
            }) => {
                assert_eq!(error.class(), crate::ErrorClass::InvalidInput);
                true
            }
            Err(_) => panic!("unexpected sender failure"),
        };
    }
    assert_eq!(refused, [false, true, true]);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn dropped_database_write_records_the_submitted_password() {
    use crate::wire::product::{evidence::Evidence, exit, options::PasswordSlot};
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let operation = operation(Method::POST, redirect::Retry::Once);
    let password = Secret::new("toy-database-password");
    let mut evidence = Evidence::new();
    evidence.add_operation_password(&password);
    let mut request = request(&server.origin);
    request.body =
        Bytes::from(serde_json::json!({"user":{"password":password.expose_secret()}}).to_string());
    crate::RequestContext::with_response_screening(async {
        let mut jitter = || 0;
        let sleeper = Immediate::default();
        let source = Provider::default();
        let source = PublicSource(&source);
        let send = super::super::run(
            Runtime {
                configured_credentials: Some(evidence.credentials()),
                client: &clients,
                ctx: &ctx,
                clock: &clock,
                deadline,
                sleeper: &sleeper,
                jitter: &mut jitter,
            },
            &operation,
            Auth::Bearer(&source),
            &request,
            RouteInputs {
                password_slot: Some(PasswordSlot::DatabaseUser(&password)),
                ..RouteInputs::default()
            },
        );
        tokio::select! {
            signal = received => signal.unwrap(),
            _ = send => panic!("write should still be pending"),
        }
        assert!(exit::recorded_exactly(password.expose_secret()));
        assert!(exit::private_safe(&password));
    })
    .await;
    assert_eq!(server.requests(), 1);
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
            response("200 OK", &[]),
        ]);
        let outcome = send_with_short_bearer(&server, bearer, "/start", &[]).await;
        assert!(matches!(
            outcome,
            Err(Failure {
                error: Error::Redirect(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 1);
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
            response("200 OK", &[]),
        ]);
        let outcome = send_with_short_bearer(&server, bearer, "/start", &[]).await;
        assert!(matches!(
            outcome,
            Err(Failure {
                error: Error::Redirect(_),
                ..
            })
        ));
        assert_eq!(server.requests(), 1);
    }
}
