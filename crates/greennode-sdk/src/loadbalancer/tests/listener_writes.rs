use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;
const CHILD: &str = r#"{"data":{"uuid":"listener-1","protocol":"HTTPS","protocolPort":443,"allowedCidrs":"10.0.0.0/24","defaultCertificateAuthority":"cert-1","progressStatus":"CREATED"}}"#;

fn create() -> CreateListenerInput {
    CreateListenerInput {
        load_balancer_id: "lb-1".into(),
        name: "web".into(),
        protocol: "HTTPS".into(),
        port: 443,
        allowed_cidrs: vec!["10.0.0.0/24".into()],
        default_certificate_id: "cert-1".into(),
        no_wait: true,
        ..Default::default()
    }
}

fn update() -> UpdateListenerInput {
    UpdateListenerInput {
        load_balancer_id: "lb-1".into(),
        listener_id: "listener-1".into(),
        timeout_client: Some(15),
        no_wait: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn invalid_listener_cidrs_and_certificates_stop_before_discovery() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, CANARY);
    for input in [
        CreateListenerInput {
            allowed_cidrs: vec!["10.0.0.1/24".into()],
            ..create()
        },
        CreateListenerInput {
            default_certificate_id: String::new(),
            ..create()
        },
        CreateListenerInput {
            protocol: "TCP".into(),
            ..create()
        },
        CreateListenerInput {
            insert_headers: vec![ListenerInsertHeader {
                header_name: "X-Route".into(),
                header_value: " toy%2Dsecret ".into(),
            }],
            ..create()
        },
    ] {
        let failure = client.create_listener(&ctx, input).await.unwrap_err();
        assert!(failure.error().has_class(ErrorClass::InvalidInput));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn listener_update_refuses_ambiguous_guard_reads_before_the_put() {
    for row in [
        r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED","protocolPort":"443"}}"#,
        r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED","allowedCidrs":"10.0.0.0/24","AllowedCidrs":"0.0.0.0/0"}}"#,
        r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED","protocol":"HTTP","allowedCidrs":"10.0.0.0/24","CertificateAuthorities":["cert-1"]}}"#,
    ] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("200 OK", row),
            response("202 Accepted", ""),
        ]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .update_listener(&ctx, update())
            .await
            .unwrap_err();
        assert_eq!(
            failure.error().detail::<DecodeError>(),
            Some(&DecodeError::InvalidShape)
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn listener_update_refuses_a_preflight_with_a_reflected_port() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("200 OK", CHILD),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "443")
        .update_listener(&ctx, update())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn listener_create_does_not_poll_an_ambiguous_acknowledgement() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "202 Accepted",
            r#"{"uuid":"listener-1","UUID":"listener-2"}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let input = CreateListenerInput {
        no_wait: false,
        ..create()
    };
    let failure = client(&server, CANARY)
        .create_listener(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn listener_confirmation_failure_keeps_the_pre_update_listener() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("200 OK", CHILD),
        response("202 Accepted", ""),
        response("403 Forbidden", r#"{"message":"lookup denied"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let input = UpdateListenerInput {
        no_wait: false,
        ..update()
    };
    let failure = client(&server, CANARY)
        .update_listener(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = &failure.partial().unwrap().listener;
    assert_eq!(partial.uuid, "listener-1");
    assert_eq!(partial.timeout_client, 0);
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn reflected_create_id_is_neither_returned_nor_polled() {
    for (token, body) in [
        (CANARY, r#"{"uuid":"ln-746f792d736563726574"}"#),
        ("abc", r#"{"uuid":"lnxYWJjtailzz"}"#),
    ] {
        for no_wait in [true, false] {
            let server = Server::start(vec![
                response("200 OK", PARENT),
                response("202 Accepted", body),
                response("403 Forbidden", "{}"),
            ]);
            let (ctx, _) = RequestContext::new();
            let input = CreateListenerInput {
                no_wait,
                ..create()
            };
            let result = client(&server, token).create_listener(&ctx, input).await;
            assert!(result.is_err());
            let failure = result.unwrap_err();
            assert!(failure.partial().is_none());
            assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
            assert_eq!(server.requests(), 2);
        }
    }
}

#[tokio::test]
async fn aliased_listener_protocol_cannot_authorize_a_merge() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"listener-1","Protocol":"HTTPS","allowedCidrs":"10.0.0.0/24","progressStatus":"CREATED"}}"#,
        ),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .update_listener(&ctx, update())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn busy_resend_uses_only_the_bounded_message() {
    for (body, resends) in [
        (
            String::from(r#"{"message":"load balancer is updat\ting"}"#),
            false,
        ),
        (
            format!(
                r#"{{"message":"{} load balancer is updating"}}"#,
                "a".repeat(550)
            ),
            false,
        ),
        (
            String::from(r#"{"message":"load balancer is updating"}"#),
            true,
        ),
    ] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("409 Conflict", &body),
            response("200 OK", PARENT),
            response("202 Accepted", r#"{"uuid":"listener-1"}"#),
        ]);
        let (ctx, _) = RequestContext::new();
        let result = client(&server, CANARY)
            .create_listener(&ctx, create())
            .await;
        assert_eq!(result.is_ok(), resends);
        assert_eq!(server.requests(), if resends { 4 } else { 2 });
    }
}

#[tokio::test]
async fn listener_pool_and_certificate_ids_use_the_strict_charset() {
    for input in [
        CreateListenerInput {
            default_pool_id: "pool_1".into(),
            ..create()
        },
        CreateListenerInput {
            certificate_ids: vec!["cert_1".into()],
            ..create()
        },
        CreateListenerInput {
            default_certificate_id: "cert_1".into(),
            ..create()
        },
        CreateListenerInput {
            client_certificate_id: "cert_1".into(),
            ..create()
        },
    ] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("202 Accepted", r#"{"uuid":"listener-1"}"#),
        ]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .create_listener(&ctx, input)
            .await
            .unwrap_err();
        assert!(failure.error().has_class(ErrorClass::InvalidInput));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 0);
    }
}
