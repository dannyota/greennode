use super::*;

#[tokio::test]
async fn starts_only_stopped_servers_and_sends_an_empty_put() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE"}}"#),
        response(r#"{"data":{"uuid":"server-1","status":"STOPPED"}}"#),
        http("202 Accepted", ""),
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE"}}"#),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let input = StartServerInput {
        server_id: "server-1".into(),
        no_wait: false,
    };
    assert!(
        !client
            .start_server(&ctx, input.clone())
            .await
            .unwrap()
            .changed
    );
    assert!(client.start_server(&ctx, input).await.unwrap().changed);
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[2].starts_with(b"PUT /v2/project-1/servers/server-1/start "));
    assert!(server.bodies()[2].is_empty());
}

#[tokio::test]
async fn stop_refuses_unsafe_preflights_and_keeps_partial_on_ambiguous_send() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1","status":"CREATING"}}"#),
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE","ignored":"toy%2Dsecret"}}"#),
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE","name":"example"}}"#),
        http(
            "502 Bad Gateway",
            r#"{"message":"provider was unavailable"}"#,
        ),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let input = StopServerInput {
        server_id: "server-1".into(),
        no_wait: false,
    };
    let refused = client.stop_server(&ctx, input.clone()).await.unwrap_err();
    assert!(refused.error().detail::<UnexpectedStatus>().is_some());
    assert_eq!(refused.write_dispatch(), crate::WriteDispatch::NotSent);
    let reflected = client.stop_server(&ctx, input.clone()).await.unwrap_err();
    assert_eq!(reflected.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(
        reflected.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    let unclear = client.stop_server(&ctx, input).await.unwrap_err();
    assert_eq!(unclear.error().class(), ErrorClass::NotSettled);
    assert_eq!(
        unclear
            .error()
            .api_error()
            .and_then(crate::ApiError::status),
        Some(502)
    );
    let advice = unclear
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(advice.contains("run this operation again to check"));
    assert!(advice.contains("provider was unavailable"));
    assert_eq!(
        unclear.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    let partial = unclear.partial().unwrap();
    assert!(!partial.changed);
    assert_eq!(partial.server.name, "example");
    assert_eq!(server.heads().len(), 4);
}

#[tokio::test]
async fn rename_rejects_array_acknowledgement() {
    let server = TestServer::start(vec![response(r#"[{"uuid":"server-1","name":"renamed"}]"#)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .rename_server(
            &ctx,
            RenameServerInput {
                server_id: "server-1".into(),
                name: "renamed".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn rename_requires_matching_acknowledgement_and_sends_new_name() {
    let server = TestServer::start(vec![response(
        r#"{"data":{"uuid":"server-2","name":"renamed"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .rename_server(
            &ctx,
            RenameServerInput {
                server_id: "server-1".into(),
                name: "renamed".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert!(failure.partial().is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[0]).unwrap(),
        serde_json::json!({"newName":"renamed"})
    );
}

#[tokio::test]
async fn resize_oversized_conflict_is_rejected() {
    let server = TestServer::start(vec![
        response(
            r#"{"data":{"uuid":"server-1","status":"ACTIVE","flavor":{"flavorId":"flavor-1"}}}"#,
        ),
        response(r#"{"optimumPrice":-20}"#),
        http("409 Conflict", &"x".repeat((8 << 20) + 1)),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .resize_server(
            &ctx,
            ResizeServerInput {
                server_id: "server-1".into(),
                flavor_id: "flavor-2".into(),
                no_wait: true,
                max_price: 0.0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(409));
    assert_ne!(failure.error().class(), ErrorClass::NotSettled);
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn resize_truncated_conflict_keeps_signed_quote_and_advice() {
    let server = TestServer::start(vec![
        response(
            r#"{"data":{"uuid":"server-1","status":"ACTIVE","flavor":{"flavorId":"flavor-1"}}}"#,
        ),
        response(r#"{"optimumPrice":-20}"#),
        Response::Http {
            status: "409 Conflict".into(),
            headers: vec![("Content-Length".into(), "10".into())],
            body: Vec::new(),
        },
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .resize_server(
            &ctx,
            ResizeServerInput {
                server_id: "server-1".into(),
                flavor_id: "flavor-2".into(),
                no_wait: true,
                max_price: 0.0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.partial().unwrap().quoted_adjustment, -20.0);
    assert!(
        failure
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message()
            .contains("GetServer")
    );
}

#[tokio::test]
async fn resize_refuses_zero_and_above_cap_before_sending_and_accepts_credit() {
    let active = || {
        response(
            r#"{"data":{"uuid":"server-1","status":"ACTIVE","flavor":{"flavorId":"flavor-1"}}}"#,
        )
    };
    let server = TestServer::start(vec![
        active(),
        response(r#"{"optimumPrice":0}"#),
        active(),
        response(r#"{"optimumPrice":20}"#),
        active(),
        response(r#"{"optimumPrice":-20}"#),
        http("202 Accepted", ""),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let input = ResizeServerInput {
        server_id: "server-1".into(),
        flavor_id: "flavor-2".into(),
        no_wait: true,
        max_price: 0.0,
    };
    for class in [ErrorClass::Unpriced, ErrorClass::PriceAboveMax] {
        let failure = client.resize_server(&ctx, input.clone()).await.unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    }
    let output = client.resize_server(&ctx, input).await.unwrap();
    assert_eq!(output.quoted_adjustment, -20.0);
    let heads = server.heads();
    assert_eq!(heads.len(), 7);
    for index in [1, 3, 5] {
        assert!(heads[index].starts_with(b"POST /portal/v1/price "));
    }
    assert!(heads[6].starts_with(b"PUT /v2/project-1/servers/server-1/resize "));
    let bodies = server.bodies();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bodies[6]).unwrap(),
        serde_json::json!({"serverId":"server-1","flavorId":"flavor-2"})
    );
}

#[test]
fn reboot_settlement_needs_active_after_the_minimum_interval() {
    use super::super::mutation::Settlement;
    use std::time::Duration;

    let acknowledged = tokio::time::Instant::now();
    let earliest = acknowledged + Duration::from_secs(10);
    let pending = Settlement::Rebooted(earliest);
    let mut server = Server {
        status: "ACTIVE".into(),
        ..Server::default()
    };
    assert!(!pending.matches(&server, acknowledged + Duration::from_secs(5)));
    assert!(pending.matches(&server, earliest));
    server.status = "REBOOTING".into();
    assert!(!pending.matches(&server, earliest));
}

#[test]
fn settlement_advice_screens_the_final_safe_text() {
    use super::super::mutation::settlement_error;
    use crate::wire::product::evidence::Evidence;
    use crate::{Error, RecoveryAdvice};

    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("toy-guidance"));
    let error = settlement_error(
        "compute.StopServer",
        Error::new(ErrorClass::Api),
        "toy-\u{202e}guidance",
        &evidence,
    );
    assert_eq!(error.class(), ErrorClass::NotSettled);
    assert!(error.detail::<RecoveryAdvice>().is_none());
}
