use super::*;

const LISTENER_READY: &str = r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED"}}"#;
const LOAD_BALANCER: &str = "/regional/v2/proj-1/loadBalancers/lb-1";

fn listener_input(no_wait: bool) -> DeleteListenerInput {
    DeleteListenerInput {
        load_balancer_id: "lb-1".into(),
        listener_id: "listener-1".into(),
        no_wait,
    }
}

#[tokio::test]
async fn pool_delete_rejects_lone_case_variants_and_collisions() {
    for (listeners, allowed) in [
        (
            r#"{"data":[{"uuid":"listener_1","defaultPoolId":"other"}]}"#,
            true,
        ),
        (
            r#"{"data":[{"uuid":"listener_1","DefaultPoolId":"ignored"}]}"#,
            false,
        ),
        (
            r#"{"data":[{"uuid":"listener_1","defaultPoolId":"other","DefaultPoolId":"ignored"}]}"#,
            false,
        ),
    ] {
        let mut replies = vec![response("200 OK", listeners)];
        if allowed {
            replies.extend([
                lb_status("CREATED"),
                response(
                    "200 OK",
                    r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED"}}"#,
                ),
                response("202 Accepted", ""),
            ]);
        }
        let server = Server::start(replies);
        let client = client(&server, CANARY);
        let (ctx, _) = RequestContext::new();
        let result = client
            .delete_pool(
                &ctx,
                DeletePoolInput {
                    load_balancer_id: "lb-1".into(),
                    pool_id: "pool-1".into(),
                    no_wait: true,
                },
            )
            .await;
        if allowed {
            assert!(result.is_ok());
            assert_eq!(server.requests(), 4);
        } else {
            let failure = result.unwrap_err();
            assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
            assert_eq!(
                failure.error().detail::<DecodeError>(),
                Some(&DecodeError::InvalidShape)
            );
            assert_eq!(server.requests(), 1);
        }
    }
}

#[tokio::test]
async fn a_busy_refusal_rereads_then_resends_the_delete_only_once() {
    let busy = || response("409 Conflict", r#"{"message":"listener is not ready"}"#);
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        busy(),
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        busy(),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_listener(&ctx, listener_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(409));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 6);
    let listener = format!("{LOAD_BALANCER}/listeners/listener-1");
    for (head, (method, path)) in server.heads().iter().zip([
        ("GET", LOAD_BALANCER),
        ("GET", &listener),
        ("DELETE", &listener),
        ("GET", LOAD_BALANCER),
        ("GET", &listener),
        ("DELETE", &listener),
    ]) {
        assert!(head.starts_with(format!("{method} {path} ").as_bytes()));
    }
}

#[tokio::test]
async fn an_incomplete_404_does_not_confirm_child_absence() {
    let truncated =
        Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\n{}".to_vec());
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
        truncated,
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_listener(&ctx, listener_input(false))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.partial(), Some(&DeleteListenerOutput {}));
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn a_complete_404_confirms_even_when_its_status_must_be_screened() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
        response("404 Not Found", "{}"),
        lb_status("CREATED"),
    ]);
    let client = client(&server, "404");
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .delete_listener(&ctx, listener_input(false))
            .await
            .is_ok()
    );
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn a_truncated_acceptance_does_not_start_confirmation() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        Response::Http {
            status: "202 Accepted".into(),
            headers: vec![("Content-Length".into(), ((8 << 20) + 1).to_string())],
            body: Vec::new(),
        },
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_listener(&ctx, listener_input(false))
        .await
        .unwrap_err();
    assert!(failure.error().is_body_too_large());
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 3);
}

const POOL_READY: &str = r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED"}}"#;

fn pool_input(no_wait: bool) -> DeletePoolInput {
    DeletePoolInput {
        load_balancer_id: "lb-1".into(),
        pool_id: "pool-1".into(),
        no_wait,
    }
}

fn pool_preflight() -> [Response; 3] {
    [
        response("200 OK", r#"{"data":[]}"#),
        lb_status("CREATED"),
        response("200 OK", POOL_READY),
    ]
}

#[tokio::test]
async fn malformed_parent_never_confirms_child_deletion() {
    let malformed = || {
        response(
            "200 OK",
            r#"{"data":{"uuid":"lb-1","progressStatus":"CREATED","internal":"no"}}"#,
        )
    };
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
        response("404 Not Found", "{}"),
        malformed(),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .delete_listener(&ctx, listener_input(false))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    assert_eq!(failure.partial(), Some(&DeleteListenerOutput {}));
    assert_eq!(
        failure
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message(),
        "vngcloud: write outcome not confirmed: loadbalancer.DeleteListener: listener listener-1: loadbalancer.GetLoadBalancer: request failed: response failed to decode"
    );
    assert_eq!(server.requests(), 5);

    let server = Server::start(vec![malformed()]);
    let failure = client(&server, CANARY)
        .delete_listener(&ctx, listener_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn conflict_arrays_require_go_error_fields_and_accept_detail_text() {
    let cases = [
        (
            r#"[{"message":"pool is updating"},{"detail":42}]"#,
            None,
            "pool is updating",
        ),
        (
            r#"[{"detail":"pool is used in listener"}]"#,
            Some(&WriteError::ResourceInUse),
            "Conflict",
        ),
    ];
    for (body, expected, message) in cases {
        let server = Server::start(
            pool_preflight()
                .into_iter()
                .chain([response("409 Conflict", body)])
                .collect(),
        );
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .delete_pool(&ctx, pool_input(true))
            .await
            .unwrap_err();
        assert_eq!(failure.error().detail::<WriteError>(), expected);
        assert_eq!(failure.error().api_error().unwrap().status(), Some(409));
        assert_eq!(failure.error().api_error().unwrap().message(), message);
        if expected.is_some() {
            let advice = failure
                .error()
                .detail::<crate::RecoveryAdvice>()
                .unwrap()
                .message();
            assert!(!advice.contains("pool is used in listener"));
        }
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 4);
    }
}

#[tokio::test]
async fn reflected_listener_id_cannot_classify_the_pool_as_in_use() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":[{"uuid":"toy-secret","defaultPoolId":"pool-1"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .delete_pool(&ctx, pool_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.error().detail::<WriteError>(), None);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn pool_delete_classifies_only_screened_messages_on_the_readiness_retry() {
    let server = Server::start(
        pool_preflight()
            .into_iter()
            .chain([
                response("409 Conflict", r#"{"message":"pool is updating"}"#),
                lb_status("CREATED"),
                response(
                    "409 Conflict",
                    r#"[{"detail":"pool is used in listener: toy%2Dsecret"}]"#,
                ),
            ])
            .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .delete_pool(&ctx, pool_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.error().detail::<WriteError>(), None);
    assert_eq!(failure.error().api_error().unwrap().status(), None);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let message = failure.error().api_error().unwrap().message();
    assert!(!message.contains(CANARY));
    assert!(!message.contains("toy%2Dsecret"));
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn child_delete_internal_bound_is_not_a_caller_deadline() {
    let (arrived, poll_started) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
        Response::StallBeforeHead(arrived),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, CANARY);
    let (result, ()) = tokio::join!(client.delete_listener(&ctx, listener_input(false)), async {
        poll_started.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(600)).await;
    });
    let failure = result.unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert!(!failure.error().has_class(ErrorClass::DeadlineExceeded));
    assert_eq!(failure.partial(), Some(&DeleteListenerOutput {}));
    let message = failure
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(message.contains("did not settle within 10m0s"));
}

#[tokio::test]
async fn busy_delete_validates_the_full_child_before_resending() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("409 Conflict", r#"{"message":"listener is not ready"}"#),
        lb_status("CREATED"),
        response(
            "200 OK",
            r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED","protocolPort":"443"}}"#,
        ),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .delete_listener(&ctx, listener_input(true))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn child_delete_does_not_retry_for_busy_text_that_reflects_a_bearer() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response(
            "409 Conflict",
            r#"{"message":"listener is not ready: toy%2Dsecret"}"#,
        ),
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("204 No Content", ""),
    ]);
    let result = client(&server, CANARY)
        .delete_listener(&RequestContext::new().0, listener_input(true))
        .await;
    assert_eq!(server.requests(), 3);
    let failure = result.unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(409));
}

#[tokio::test]
async fn child_delete_cannot_confirm_absence_from_an_oversized_404() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
        Server::oversized("404 Not Found"),
        lb_status("CREATED"),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .delete_listener(&ctx, listener_input(false))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert_eq!(
        failure.error().http_status(),
        Some(http::StatusCode::NOT_FOUND)
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn child_delete_rechecks_a_busy_receipt_after_readiness() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response(
            "409 Conflict",
            r#"{"message":"listener is not ready","unknown":"confirmation-bearer"}"#,
        ),
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", ""),
    ]);
    let failure = super::receipt_fixtures::rotating_client(&server, 3)
        .delete_listener(&RequestContext::new().0, listener_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 5);
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
}

#[tokio::test]
async fn no_wait_child_delete_refuses_transport_reflection_in_accepted_body() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("200 OK", LISTENER_READY),
        response("202 Accepted", r#"{"ignored":"toy-secret"}"#),
    ]);
    let failure = client(&server, CANARY)
        .delete_listener(&RequestContext::new().0, listener_input(true))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(server.requests(), 3);
}
