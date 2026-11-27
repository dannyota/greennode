use super::*;

#[tokio::test]
async fn group_update_merges_omitted_name_and_retains_partial_on_missing_confirmation() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"group-1","name":"group","description":"old"}}"#),
        response(""),
        http("404 Not Found", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .update_server_group(
            &ctx,
            UpdateServerGroupInput {
                server_group_id: "group-1".into(),
                description: Some(String::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(output.error().class(), ErrorClass::NotSettled);
    assert!(output.error().has_class(ErrorClass::NotFound));
    assert_eq!(output.partial().unwrap().server_group.name, "group");
    assert!(server.heads()[1].starts_with(b"PUT /v2/project-1/serverGroups/group-1 "));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[1]).unwrap(),
        serde_json::json!({"name":"group","description":"","serverGroupId":"group-1"})
    );
}

#[tokio::test]
async fn groups_refuse_attached_members_and_preserve_provider_in_use_refusal() {
    let group_page = |members: &str| {
        response(&format!(
            r#"{{"listData":[{{"uuid":"group-1","name":"group","servers":{members}}}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}}"#
        ))
    };
    let server = TestServer::start(vec![
        group_page(r#"[{"uuid":"server-1"}]"#),
        group_page("[]"),
        http("409 Conflict", r#"{"message":"SERVER GROUP IS IN USE"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let input = DeleteServerGroupInput {
        server_group_id: "group-1".into(),
    };
    let refusal = client
        .delete_server_group(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(refusal.write_dispatch(), crate::WriteDispatch::NotSent);
    assert!(refusal.error().detail::<ServerGroupInUse>().is_some());
    let provider = client.delete_server_group(&ctx, input).await.unwrap_err();
    assert_eq!(
        provider.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert!(provider.error().detail::<ServerGroupInUse>().is_some());
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[2].starts_with(b"DELETE /v2/project-1/serverGroups/group-1 "));
}

#[tokio::test]
async fn group_create_truncated_conflict_keeps_recovery_advice() {
    let server = TestServer::start(vec![Response::Http {
        status: "409 Conflict".into(),
        headers: vec![("Content-Length".into(), "10".into())],
        body: Vec::new(),
    }]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_server_group(
            &ctx,
            CreateServerGroupInput {
                name: "group".into(),
                policy_id: "policy-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert!(failure.error().detail::<crate::RecoveryAdvice>().is_some());
}

#[tokio::test]
async fn group_create_oversized_conflict_is_rejected() {
    let server = TestServer::start(vec![http("409 Conflict", &"x".repeat((8 << 20) + 1))]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_server_group(
            &ctx,
            CreateServerGroupInput {
                name: "group".into(),
                policy_id: "policy-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(409));
    assert!(failure.error().detail::<crate::RecoveryAdvice>().is_none());
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
}

#[tokio::test]
async fn created_group_rejects_encoded_credentials_in_all_identifiers() {
    for body in [
        r#"{"data":{"uuid":"group-1","policyId":"preYWJjpost"}}"#,
        r#"{"data":{"uuid":"group-1","servers":[{"uuid":"preYWJjpost"}]}}"#,
        r#"{"data":{"uuid":"group-1","serverGroupId":"preYWJjpost"}}"#,
    ] {
        let server = TestServer::start(vec![http("201 Created", body)]);
        let (ctx, _) = RequestContext::new();
        let result = client_with_token(&server, "abc")
            .create_server_group(
                &ctx,
                CreateServerGroupInput {
                    name: "group".into(),
                    policy_id: "policy-1".into(),
                    ..Default::default()
                },
            )
            .await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn created_group_rejects_encoded_short_bearer_in_identifier() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"uuid":"preYWJjpost","name":"group"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let result = client_with_token(&server, "abc")
        .create_server_group(
            &ctx,
            CreateServerGroupInput {
                name: "group".into(),
                policy_id: "policy-1".into(),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn group_update_withholds_id_matching_confirmation_bearer() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Provider(AtomicUsize);
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                Ok(crate::Token::new(
                    Secret::new(if self.0.fetch_add(1, Ordering::SeqCst) >= 2 {
                        "abc"
                    } else {
                        "initial-token"
                    }),
                    None,
                ))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"preYWJjpost","name":"group"}}"#),
        response(""),
        response(r#"{"data":{"uuid":"preYWJjpost","name":"new"}}"#),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_credentials_provider(Arc::new(Provider(AtomicUsize::new(0))))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .update_server_group(
            &ctx,
            UpdateServerGroupInput {
                server_group_id: "preYWJjpost".into(),
                name: Some("new".into()),
                ..Default::default()
            },
        )
        .await;
    match result {
        Ok(_) => panic!("credential-bearing identifier was returned"),
        Err(failure) => assert!(failure.partial().is_none()),
    }
}

#[tokio::test]
async fn group_delete_rejects_encoded_credential_in_inventory_id() {
    let server = TestServer::start(vec![
        response(
            r#"{"listData":[{"uuid":"preYWJjpost","servers":[]}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
        ),
        http("204 No Content", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client_with_token(&server, "abc")
        .delete_server_group(
            &ctx,
            DeleteServerGroupInput {
                server_group_id: "group-1".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn created_group_uses_201_and_preserves_non_path_provider_id() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"uuid":"group_1","name":"group"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .create_server_group(
            &ctx,
            CreateServerGroupInput {
                name: "group".into(),
                policy_id: "policy-1".into(),
                description: "example".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.server_group.uuid, "group_1");
    assert!(server.heads()[0].starts_with(b"POST /v2/project-1/serverGroups "));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[0]).unwrap(),
        serde_json::json!({"name":"group","policyId":"policy-1","description":"example"})
    );
}

#[tokio::test]
async fn group_writes_screen_raw_preflight_keys_before_json_conversion() {
    let server = TestServer::start(vec![
        response(
            r#"{"data":{"uuid":"group-1","name":"group","ignored":{"$serde_json::private::Number":"88"}}}"#,
        ),
        response(
            r#"{"listData":[{"uuid":"group-1","servers":[],"ignored":{"$serde_json::private::Number":"88"}}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client_with_token(&server, "private");
    let update = client
        .update_server_group(
            &ctx,
            UpdateServerGroupInput {
                server_group_id: "group-1".into(),
                description: Some("new-description".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let delete = client
        .delete_server_group(
            &ctx,
            DeleteServerGroupInput {
                server_group_id: "group-1".into(),
            },
        )
        .await
        .unwrap_err();
    for (error, dispatch) in [
        (update.error(), update.write_dispatch()),
        (delete.error(), delete.write_dispatch()),
    ] {
        assert_eq!(dispatch, crate::WriteDispatch::NotSent);
        assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
    }
    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(
        heads
            .iter()
            .all(|head| head.starts_with(b"GET /v2/project-1/serverGroups"))
    );
}

#[tokio::test]
async fn group_update_guards_the_read_after_dispatch() {
    for (note, allowed) in [("ordinary-note", true), (CANARY, false)] {
        let post = serde_json::json!({"data":{"uuid":"group-1","name":"new-group","note":note}})
            .to_string();
        let server = TestServer::start(vec![
            response(r#"{"data":{"uuid":"group-1","name":"group"}}"#),
            response(""),
            response(&post),
        ]);
        let outcome = client(&server)
            .update_server_group(
                &RequestContext::new().0,
                UpdateServerGroupInput {
                    server_group_id: "group-1".into(),
                    name: Some("new-group".into()),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(outcome.is_ok(), allowed);
        if let Err(failure) = outcome {
            assert_eq!(
                failure.write_dispatch(),
                crate::WriteDispatch::MayHaveBeenSent
            );
        }
        assert_eq!(server.heads().len(), 3);
    }
}
