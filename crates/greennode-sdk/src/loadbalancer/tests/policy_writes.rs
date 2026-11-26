use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;

fn update() -> UpdatePolicyInput {
    UpdatePolicyInput {
        load_balancer_id: "lb-1".into(),
        listener_id: "listener-1".into(),
        policy_id: "policy-1".into(),
        redirect_pool_id: Some("pool-2".into()),
        no_wait: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn policy_update_refuses_to_resend_incomplete_read_rules() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"policy-1","action":"REDIRECT_TO_POOL","redirectPoolId":"pool-1","progressStatus":"CREATED","l7Rules":[{"ruleType":"PATH","compareType":"EQUAL_TO","RuleValue":"/api"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .update_policy(&ctx, update())
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
async fn policy_update_projects_reflected_position_without_leaking_the_number() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"policy-1","action":"REDIRECT_TO_POOL","redirectPoolId":"pool-1","progressStatus":"CREATED","position":42,"l7Rules":[{"ruleType":"PATH","compareType":"EQUAL_TO","ruleValue":"/api"}]}}"#,
        ),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, "42")
        .update_policy(&ctx, update())
        .await
        .unwrap();
    assert_eq!(output.policy.position, 0);
    assert_eq!(
        serde_json::to_value(output).unwrap()["Policy"]["Position"],
        "<redacted>"
    );
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn policy_create_refuses_forbidden_action_fields_before_any_read() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreatePolicyInput {
        load_balancer_id: "lb-1".into(),
        listener_id: "listener-1".into(),
        name: "routing".into(),
        action: "REDIRECT_TO_POOL".into(),
        redirect_pool_id: "pool-1".into(),
        redirect_url: "https://example.test/".into(),
        rules: vec![PolicyRuleInput {
            r#type: "PATH".into(),
            compare_type: "EQUAL_TO".into(),
            value: "/api".into(),
        }],
        ..Default::default()
    };
    let client = client(&server, CANARY);
    let failure = client.create_policy(&ctx, input.clone()).await.unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    input.redirect_url.clear();
    input.rules[0].value.clear();
    let failure = client.create_policy(&ctx, input).await.unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn aliased_policy_action_cannot_authorize_a_merge() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response(
            "200 OK",
            r#"{"data":{"uuid":"policy-1","Action":"REDIRECT_TO_URL","progressStatus":"CREATED","l7Rules":[{"ruleType":"PATH","compareType":"EQUAL_TO","ruleValue":"/api"}]}}"#,
        ),
        response("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .update_policy(&ctx, update())
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
async fn policy_update_rejects_aliases_in_carried_fields() {
    for row in [
        r#"{"data":{"uuid":"policy-1","action":"REDIRECT_TO_POOL","RedirectUrl":"https://example.test/","progressStatus":"CREATED","l7Rules":[{"ruleType":"PATH","compareType":"EQUAL_TO","ruleValue":"/api"}]}}"#,
        r#"{"data":{"uuid":"policy-1","action":"REDIRECT_TO_POOL","progressStatus":"CREATED","l7Rules":[{"ruleType":"PATH","compareType":"EQUAL_TO","ruleValue":"/api","RuleValue":"/other"}]}}"#,
    ] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("200 OK", row),
            response("202 Accepted", ""),
        ]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .update_policy(&ctx, update())
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
async fn policy_redirect_pool_id_uses_the_strict_charset() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"policy-1"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let input = CreatePolicyInput {
        load_balancer_id: "lb-1".into(),
        listener_id: "listener-1".into(),
        name: "routing".into(),
        action: "REDIRECT_TO_POOL".into(),
        redirect_pool_id: "pool_1".into(),
        rules: vec![PolicyRuleInput {
            r#type: "PATH".into(),
            compare_type: "EQUAL_TO".into(),
            value: "/api".into(),
        }],
        no_wait: true,
        ..Default::default()
    };
    let failure = client(&server, CANARY)
        .create_policy(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}
