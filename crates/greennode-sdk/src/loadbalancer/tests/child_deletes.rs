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
async fn pool_delete_ignores_lone_case_variants_and_refuses_collisions() {
    for (listeners, allowed) in [
        (
            r#"{"data":[{"uuid":"listener_1","DefaultPoolId":"ignored"}]}"#,
            true,
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
    let busy = || {
        response(
            "409 Conflict",
            r#"{"message":"listener is not ready: toy%2Dsecret"}"#,
        )
    };
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
    let truncated = Response::Http {
        status: "404 Not Found".into(),
        headers: vec![("Content-Length".into(), ((8 << 20) + 1).to_string())],
        body: Vec::new(),
    };
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
