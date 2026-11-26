use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;
const MONITOR: &str = r#"{"data":{"healthCheckProtocol":"HTTP","healthyThreshold":3,"unhealthyThreshold":3,"interval":30,"timeout":5}}"#;
const POOL_ROUTE: &str = "/regional/v2/proj-1/loadBalancers/lb-1/pools/pool-1";

fn create(no_wait: bool) -> CreatePoolInput {
    CreatePoolInput {
        load_balancer_id: "lb-1".into(),
        name: "web".into(),
        protocol: "HTTP".into(),
        health_check_protocol: "HTTP".into(),
        no_wait,
        ..Default::default()
    }
}

fn update() -> UpdatePoolInput {
    UpdatePoolInput {
        load_balancer_id: "lb-1".into(),
        pool_id: "pool-1".into(),
        algorithm: Some("ROUND_ROBIN".into()),
        no_wait: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn reflected_member_data_is_redacted_in_a_submitted_update_output() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"pool-1","protocol":"HTTP","progressStatus":"CREATED","members":[{"uuid":"member_1","name":"value42","weight":42}]}}"#,
        ),
        response("200 OK", MONITOR),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, "42")
        .update_pool(&ctx, update())
        .await
        .unwrap();
    assert!(!output.pool.members[0].name.contains("42"));
    assert_ne!(output.pool.members[0].weight, 42);
    let projected = serde_json::to_value(output).unwrap();
    assert_eq!(projected["Pool"]["Members"][0]["Name"], "<redacted>");
    assert_eq!(projected["Pool"]["Members"][0]["Weight"], "<redacted>");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn pool_create_and_update_use_different_transport_retries() {
    let create_server = Server::start(vec![
        response("200 OK", PARENT),
        response("502 Bad Gateway", r#"{"message":"pool is updating"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&create_server, CANARY)
        .create_pool(&ctx, create(true))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(
        failure
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message()
            .contains("check with list-pools")
    );
    assert_eq!(create_server.requests(), 2);

    let update_server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"pool-1","protocol":"HTTP","progressStatus":"CREATED"}}"#,
        ),
        response("200 OK", MONITOR),
        response("502 Bad Gateway", "{}"),
        response("202 Accepted", ""),
    ]);
    let output = client(&update_server, CANARY)
        .update_pool(&ctx, update())
        .await
        .unwrap();
    assert_eq!(output.pool.uuid, "pool-1");
    assert_eq!(update_server.requests(), 5);
    let bodies = update_server.bodies();
    assert!(bodies[3] == bodies[4]);
    assert!(update_server.heads()[4].starts_with(format!("PUT {POOL_ROUTE} ").as_bytes()));
}

#[tokio::test]
async fn ambiguous_pool_acknowledgements_never_start_confirmation() {
    for body in [
        r#"{"UUID":"pool-1"}"#,
        r#"{"uuid":"pool-1","UUID":"pool-2"}"#,
        r#"{"uuid":"../pool-1"}"#,
        r#"{"uuid":"pool_1"}"#,
    ] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("202 Accepted", body),
        ]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .create_pool(&ctx, create(false))
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().is_none());
        assert_eq!(server.requests(), 2);
    }
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"pool_1"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, CANARY)
        .create_pool(&ctx, create(true))
        .await
        .unwrap();
    assert_eq!(output.pool.uuid, "pool_1");
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn credential_in_caller_input_blocks_the_first_read() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let mut input = create(true);
    input.health_check_path = "/toy%2Dsecret".into();
    let failure = client(&server, CANARY)
        .create_pool(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn pool_confirmation_keeps_submitted_or_read_state_on_failure() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"pool-1"}"#),
        response("403 Forbidden", r#"{"message":"lookup denied"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .create_pool(&ctx, create(false))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let pool = &failure.partial().unwrap().pool;
    assert_eq!(pool.uuid, "pool-1");
    assert_eq!(pool.load_balance_method, "ROUND_ROBIN");
    assert_eq!(pool.health_monitor.as_ref().unwrap().interval, 30);
    assert_eq!(server.requests(), 3);

    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"pool-1","protocol":"HTTP","loadBalanceMethod":"LEAST_CONNECTIONS","progressStatus":"CREATED"}}"#,
        ),
        response("200 OK", MONITOR),
        response("202 Accepted", ""),
        response(
            "200 OK",
            r#"{"data":{"uuid":"pool-1","protocol":"HTTP","loadBalanceMethod":"ROUND_ROBIN","progressStatus":"ERROR"}}"#,
        ),
    ]);
    let mut input = update();
    input.no_wait = false;
    let failure = client(&server, CANARY)
        .update_pool(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::Failed));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let pool = &failure.partial().unwrap().pool;
    assert_eq!(pool.progress_status, "ERROR");
    assert_eq!(pool.load_balance_method, "ROUND_ROBIN");
    assert_eq!(server.requests(), 5);
}
