use super::*;
use crate::WriteDispatch;

fn session(replies: Vec<Response>) -> (Server, Client, RequestContext) {
    let server = Server::start(replies);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    (server, Client::new(&config), RequestContext::new().0)
}

#[tokio::test]
async fn group_delete_refuses_case_variant_inventory() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"id":"sg-1","system":false}}"#),
        response("200 OK", r#"{"Data":[{"uuid":"server-1"}]}"#),
    ]);
    let error = client
        .delete_security_group(&ctx, DeleteSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn group_delete_checks_membership_before_sending() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"id":"sg-1","system":false}}"#),
        response("200 OK", r#"{"data":[]}"#),
        response("204 No Content", ""),
    ]);
    client
        .delete_security_group(&ctx, DeleteSecurityGroupInput::new("sg-1"))
        .await
        .unwrap();
    let heads = server.heads();
    assert!(heads[1].starts_with(b"GET /v2/proj-1/secgroups/sg-1/servers "));
    assert!(heads[2].starts_with(b"DELETE /v2/proj-1/secgroups/sg-1 "));
}

#[tokio::test]
async fn group_create_uses_confirmed_group() {
    let (server, client, ctx) = session(vec![
        response(
            "201 Created",
            r#"{"data":{"id":10,"uuid":"sg-1","secgroupName":"test"}}"#,
        ),
        response(
            "200 OK",
            r#"{"data":{"id":"sg-1","name":"test","status":"ACTIVE"}}"#,
        ),
    ]);
    let output = client
        .create_security_group(&ctx, CreateSecurityGroupInput::new("test"))
        .await
        .unwrap();
    assert_eq!(output.security_group.status, "ACTIVE");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"POST /v2/proj-1/secgroups "));
    assert!(heads[1].starts_with(b"GET /v2/proj-1/secgroups/sg-1 "));
}

#[tokio::test]
async fn vip_delete_refuses_case_variant_address_pairs() {
    let (server, client, ctx) = session(vec![response(
        "200 OK",
        r#"{"data":{"uuid":"vip-1","type":"private","AddressPairIps":["192.0.2.1"]}}"#,
    )]);
    let error = client
        .delete_virtual_ip_address(&ctx, DeleteVirtualIpAddressInput::new("vip-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn subnet_delete_guard_refuses_server_field_variants() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"[{"UUID":"server-1"}]"#),
        response("200 OK", r#"[{"uuid":"server-1"}]"#),
    ]);
    let mut evidence = client.request_evidence(&ctx).unwrap();
    assert!(
        client
            .guard_servers_by_subnet(&ctx, "sub-1", &mut evidence)
            .await
            .is_err()
    );
    let rows = client
        .guard_servers_by_subnet(&ctx, "sub-1", &mut evidence)
        .await
        .unwrap();
    assert_eq!(rows.items.len(), 1);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET /v2/proj-1/servers/subnets/sub-1 "))
    );
}

#[tokio::test]
async fn group_delete_refuses_attached_servers() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"id":"sg-1","system":false}}"#),
        response("200 OK", r#"{"data":[{"uuid":"server-1"}]}"#),
    ]);
    let error = client
        .delete_security_group(&ctx, DeleteSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert!(error.error().detail::<SecurityGroupInUse>().is_some());
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn group_create_does_not_poll_an_ambiguous_identifier() {
    let (server, client, ctx) = session(vec![response(
        "201 Created",
        r#"{"data":{"uuid":"sg-1","UUID":"sg-2","secgroupName":"test"}}"#,
    )]);
    let error = client
        .create_security_group(&ctx, CreateSecurityGroupInput::new("test"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert!(error.partial().is_none());
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn vip_update_retries_503_with_the_same_resolved_body() {
    let (server, client, ctx) = session(vec![
        response(
            "200 OK",
            r#"{"data":{"uuid":"vip-1","name":"original","description":"keep","mode":"Active/Passive"}}"#,
        ),
        response("503 Service Unavailable", r#"{"message":"try later"}"#),
        response("200 OK", r#"{"data":{"uuid":"vip-1","name":"original"}}"#),
        response(
            "200 OK",
            r#"{"data":{"uuid":"vip-1","name":"original","description":"","mode":"Active/Passive"}}"#,
        ),
    ]);
    let mut input = UpdateVirtualIpAddressInput::new("vip-1");
    input.description = Some(String::new());
    client.update_virtual_ip_address(&ctx, input).await.unwrap();
    assert_eq!(server.heads().len(), 4);
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(
        body == serde_json::json!({"name":"original","description":"","mode":"Active/Passive"})
    );
    assert!(bodies[1] == bodies[2]);
}

#[tokio::test]
async fn group_update_refuses_system_group_before_mutation() {
    let (server, client, ctx) = session(vec![response(
        "200 OK",
        r#"{"data":{"id":"sg-1","system":true}}"#,
    )]);
    let mut input = UpdateSecurityGroupInput::new("sg-1");
    input.name = Some("renamed".into());
    let error = client.update_security_group(&ctx, input).await.unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert!(error.error().detail::<SystemSecurityGroup>().is_some());
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn group_update_keeps_safe_partial_after_a_truncated_confirmation() {
    let (server, client, ctx) = session(vec![
        response(
            "200 OK",
            r#"{"data":{"id":"sg-1","name":"old","description":"keep"}}"#,
        ),
        response("200 OK", ""),
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), "100".into())],
            body: b"{}".to_vec(),
        },
    ]);
    let mut input = UpdateSecurityGroupInput::new("sg-1");
    input.name = Some("renamed".into());
    let error = client.update_security_group(&ctx, input).await.unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().class(), ErrorClass::NotSettled);
    let group = &error.partial().unwrap().security_group;
    assert!(group.id == "sg-1" && group.name == "renamed" && group.description == "keep");
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn vip_delete_refuses_a_listed_address_pair_before_mutation() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"uuid":"vip-1","type":"private"}}"#),
        response("200 OK", r#"{"data":[{"uuid":"pair-1"}]}"#),
    ]);
    let error = client
        .delete_virtual_ip_address(&ctx, DeleteVirtualIpAddressInput::new("vip-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert!(error.error().detail::<ResourceInUse>().is_some());
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn rule_delete_refuses_a_rule_outside_its_group() {
    let (server, client, ctx) = session(vec![response("200 OK", r#"{"data":[{"id":"rule-2"}]}"#)]);
    let input = DeleteSecurityGroupRuleInput::new("sg-1", "rule-1");
    let error = client
        .delete_security_group_rule(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn group_delete_wraps_provider_in_use_refusal() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"id":"sg-1","system":false}}"#),
        response("200 OK", r#"{"data":[]}"#),
        response(
            "409 Conflict",
            r#"{"message":"SecurityGroupInUse: group is attached to a resource"}"#,
        ),
    ]);
    let error = client
        .delete_security_group(&ctx, DeleteSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.error().detail::<SecurityGroupInUse>().is_some());
    assert_eq!(
        error.error().api_error().unwrap().message(),
        "network: security group in use: network.DeleteSecurityGroup: SecurityGroupInUse: group is attached to a resource (status 409)"
    );
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn group_delete_does_not_classify_truncated_membership_as_http_refusal() {
    let (server, client, ctx) = session(vec![
        response("200 OK", r#"{"data":{"id":"sg-1","system":false}}"#),
        Response::Http {
            status: "404 Not Found".into(),
            headers: vec![("Content-Length".into(), "100".into())],
            body: b"{}".to_vec(),
        },
    ]);
    let error = client
        .delete_security_group(&ctx, DeleteSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        error.error().api_error().and_then(crate::ApiError::status),
        None
    );
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn vip_guard_refuses_alias_inside_zone() {
    let (server, client, ctx) = session(vec![response(
        "200 OK",
        r#"{"data":{"uuid":"vip-1","type":"private","zone":{"UUID":"zone-1"}}}"#,
    )]);
    let error = client
        .delete_virtual_ip_address(&ctx, DeleteVirtualIpAddressInput::new("vip-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn vip_create_counts_original_bytes_before_lossy_decoding() {
    let mut body = br#"{"data":{"uuid":"vip-1","status":"ACTIVE","unused":""#.to_vec();
    body.resize(body.len() + (3 << 20), 0xff);
    body.extend_from_slice(br#""}}"#);
    let (server, client, ctx) = session(vec![Response::Http {
        status: "201 Created".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }]);
    let output = client
        .create_virtual_ip_address(
            &ctx,
            CreateVirtualIpAddressInput::new("sub-1", "vip", "Active/Active"),
        )
        .await
        .unwrap();
    assert_eq!(output.virtual_ip_address.uuid, "vip-1");
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn ambiguous_group_create_keeps_api_cause_in_recovery_message() {
    let (server, client, ctx) = session(vec![response(
        "502 Bad Gateway",
        r#"{"message":"upstream failure"}"#,
    )]);
    let error = client
        .create_security_group(&ctx, CreateSecurityGroupInput::new("name"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().api_error().unwrap().status(), Some(502));
    let advice = error
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(advice.contains("network.CreateSecurityGroup: upstream failure (status 502)"));
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn group_create_retries_429_and_never_retries_gateway_errors() {
    for (status, retry) in [
        ("429 Too Many Requests", true),
        ("503 Service Unavailable", false),
        ("500 Internal Server Error", false),
    ] {
        let (server, client, ctx) = session(vec![
            response(status, "{}"),
            response(
                "201 Created",
                r#"{"data":{"uuid":"sg-1","secgroupName":"group"}}"#,
            ),
        ]);
        let mut input = CreateSecurityGroupInput::new("group");
        input.no_wait = true;
        let result = client.create_security_group(&ctx, input).await;
        assert_eq!(result.is_ok(), retry, "{status}");
        assert_eq!(server.heads().len(), if retry { 2 } else { 1 });
    }
}

#[test]
fn update_confirmation_deadline_reports_the_confirmation_timeout() {
    use super::super::mutations::settlement;
    let failure: crate::Failure<()> = settlement(
        "network.UpdateSecurityGroup",
        "security group",
        "sg-1",
        crate::ContextEnd::DeadlineExceeded.into(),
        None,
        &crate::wire::product::evidence::Evidence::new(),
    );
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        "vngcloud: write outcome not confirmed: network.UpdateSecurityGroup: security group sg-1: context deadline exceeded"
    );
    assert!(failure.error().has_class(ErrorClass::DeadlineExceeded));
}

#[tokio::test]
async fn group_delete_rechecks_an_earlier_guard_with_the_final_read_credential() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{future::Future, pin::Pin, sync::Arc};

    struct Tokens(AtomicUsize);
    impl crate::CredentialsProvider for Tokens {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            let token = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                "first-toy-token"
            } else {
                "later-toy-token"
            };
            Box::pin(async move { Ok(crate::Token::new(Secret::new(token), None)) })
        }

        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"id":"sg-1","system":false},"unmodeled":"later-toy-token"}"#,
        ),
        response("200 OK", r#"{"data":[]}"#),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(Arc::new(Tokens(AtomicUsize::new(0))));
    let config = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let failure = Client::new(&config)
        .delete_security_group(
            &RequestContext::new().0,
            DeleteSecurityGroupInput::new("sg-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.requests(), 2);
}
