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
