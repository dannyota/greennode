use super::*;
use crate::testing::{Product, with_product_loopback, with_write_loopback};
use serde_json::{Value, json};

const ROOT: &str = "/glb-controller/v1/global-load-balancers/glb-1";
const POOLS: &str = "/global-pools";
const POOL: &str = "/global-pools/pool-1";
const GROUPS: &str = "/global-pools/pool-1/pool-members";
const GROUP: &str = "/global-pools/pool-1/pool-members/group-1";
const LISTENERS: &str = "/global-listeners";
const LISTENER: &str = "/global-listeners/listener-1";

fn child_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/globalloadbalancer/child_writes.json"
    ))
    .unwrap()
}

fn ok(value: &Value) -> Response {
    response("200 OK", value.to_string())
}
fn accepted(value: &Value) -> Response {
    response("202 Accepted", value.to_string())
}
fn empty() -> Response {
    response("200 OK", "[]")
}
fn gone() -> Response {
    response("404 Not Found", "{}")
}
fn internal() -> Response {
    response("500 Internal Server Error", r#"{"code":"internal_error"}"#)
}

fn servers(
    glb: Vec<Response>,
    regional: Vec<Response>,
    project: bool,
    token: &str,
) -> (Client, Server, Server) {
    let glb = Server::start(glb);
    let regional = Server::start(regional);
    let mut options = ConfigOptions::default()
        .with_region("han-1".into())
        .with_static_token(Secret::new(token))
        .with_endpoint_overrides(EndpointOverrides {
            global_load_balancer: Some(format!("{}/glb-controller/", glb.origin)),
            ..Default::default()
        });
    if project {
        options = options.with_project_id("project-1".into());
    }
    let options = with_write_loopback(Product::GlobalLoadBalancer, options, &glb);
    let options = with_product_loopback(Product::Vserver, options, &regional);
    (Client::new(&Config::new(options).unwrap()), glb, regional)
}

fn catalog(f: &Value) -> Vec<Response> {
    vec![ok(&f["parent"]), ok(&f["regions"]), ok(&f["packages"])]
}
fn chain(f: &Value) -> Vec<Response> {
    vec![
        ok(&json!([f["pool"]])),
        ok(&f["pool"]),
        ok(&json!([f["group"]])),
        ok(&f["group"]),
    ]
}
fn placement(f: &Value) -> Vec<Response> {
    vec![ok(&f["server"]), ok(&f["vpc"]), ok(&f["subnet"])]
}
fn pool_input() -> CreatePoolInput {
    CreatePoolInput::new(
        "glb-1",
        "demo-pool",
        "backend",
        "project-1",
        "server-1",
        "vpc-1",
        "sub-1",
        8080,
    )
}
fn listener_input() -> CreateListenerInput {
    CreateListenerInput::new(
        "glb-1",
        "demo-listener",
        "pool-1",
        443,
        vec!["192.0.2.1/32".into(), "198.51.100.2/32".into()],
        "project-1",
        "server-1",
    )
}

fn paths(server: &Server, expected: &[(&str, &str)]) {
    let heads = server.heads();
    assert_eq!(heads.len(), expected.len());
    for (head, (method, path)) in heads.iter().zip(expected) {
        assert!(head.starts_with(format!("{method} {path} ").as_bytes()));
    }
}

#[tokio::test]
async fn pool_creation_discovers_project_and_uses_catalog_region_for_private_backend() {
    let f = child_fixture();
    let mut glb = catalog(&f);
    glb.extend([
        empty(),
        empty(),
        accepted(&f["pool_accept"]),
        ok(&f["parent"]),
    ]);
    glb.extend(chain(&f));
    glb.push(empty());
    let mut regional = vec![ok(&f["projects"])];
    regional.extend(placement(&f));
    let (client, glb, regional) = servers(glb, regional, false, TOKEN);
    let (ctx, _) = RequestContext::new();
    let output = client.create_pool(&ctx, pool_input()).await.unwrap();
    assert_eq!(output.pool.id, "pool-1");
    assert_eq!(output.pool_member.region, "han_lab");
    assert_eq!(output.pool_member.members[0].id, "row-1");
    paths(
        &regional,
        &[
            ("GET", "/v1/projects"),
            ("GET", "/v2/project-1/servers/server-1"),
            ("GET", "/v2/project-1/networks/vpc-1"),
            ("GET", "/v2/project-1/networks/vpc-1/subnets/sub-1"),
        ],
    );
    let suffixes = [
        POOLS, LISTENERS, POOLS, "", POOLS, POOL, GROUPS, GROUP, LISTENERS,
    ];
    let mut expected = vec![
        ("GET", ROOT.to_owned()),
        ("GET", "/glb-controller/v1/regions".into()),
        ("GET", "/glb-controller/v1/packages".into()),
    ];
    expected.extend(
        suffixes
            .into_iter()
            .enumerate()
            .map(|(i, s)| (if i == 2 { "POST" } else { "GET" }, format!("{ROOT}{s}"))),
    );
    paths(
        &glb,
        &expected
            .iter()
            .map(|(m, p)| (*m, p.as_str()))
            .collect::<Vec<_>>(),
    );
    let body = serde_json::from_slice::<Value>(&glb.bodies()[5]).ok();
    let want = json!({"name":"demo-pool","algorithm":"ROUND_ROBIN","protocol":"TCP","health":{"protocol":"TCP","healthyThreshold":3,"unhealthyThreshold":3,"interval":30,"timeout":5},"globalPoolMembers":[{"name":"backend","region":"han_lab","type":"PRIVATE","vpcId":"vpc-1","trafficDial":100,"members":[{"name":"backend","address":"10.1.2.3","subnetId":"sub-1","port":8080,"monitorPort":8080,"weight":1,"backupRole":false}]}]});
    assert!(body == Some(want));
}

#[tokio::test]
async fn listener_creation_confirms_the_exact_cidr_set_and_same_backend() {
    let mut f = child_fixture();
    f["listener"]["allowedCidrs"] = json!("198.51.100.2/32,192.0.2.1/32");
    let mut glb = catalog(&f);
    glb.push(empty());
    glb.extend(chain(&f));
    glb.extend([
        accepted(&f["listener_accept"]),
        ok(&f["listener"]),
        ok(&json!([f["listener"]])),
    ]);
    glb.extend(chain(&f));
    let (client, glb, regional) = servers(glb, placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    assert_eq!(
        client
            .create_listener(&ctx, listener_input())
            .await
            .unwrap()
            .listener
            .id,
        "listener-1"
    );
    let head = glb.heads();
    assert!(head[8].starts_with(format!("POST {ROOT}{LISTENERS} ").as_bytes()));
    assert!(head[9].starts_with(format!("GET {ROOT}{LISTENER} ").as_bytes()));
    let body = serde_json::from_slice::<Value>(&glb.bodies()[8]).ok();
    assert!(
        body == Some(
            json!({"name":"demo-listener","protocol":"TCP","port":443,"globalPoolId":"pool-1","allowedCidrs":"192.0.2.1/32,198.51.100.2/32","timeoutClient":50,"timeoutConnection":5,"timeoutMember":50})
        )
    );
    assert_eq!(regional.requests(), 3);
    assert_eq!(glb.requests(), 15);
}

#[tokio::test]
async fn delete_listener_confirms_list_and_detail_then_preserves_pool_and_group() {
    let f = child_fixture();
    let mut glb = catalog(&f);
    glb.extend([ok(&f["listener"]), ok(&json!([f["listener"]]))]);
    glb.extend(chain(&f));
    glb.extend([
        response("202 Accepted", ""),
        empty(),
        gone(),
        ok(&f["parent"]),
        ok(&json!([f["pool"]])),
        ok(&f["pool"]),
        ok(&json!([f["group"]])),
    ]);
    let (client, glb, regional) = servers(glb, vec![], true, TOKEN);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .delete_listener(&ctx, DeleteListenerInput::new("glb-1", "listener-1"))
            .await
            .unwrap()
            .deleted
    );
    assert!(glb.heads()[9].starts_with(format!("DELETE {ROOT}{LISTENER} ").as_bytes()));
    assert_eq!(glb.requests(), 16);
    assert_eq!(regional.requests(), 0);
}

#[tokio::test]
async fn delete_pool_accepts_complete_internal_error_only_after_pool_disappears() {
    let f = child_fixture();
    for code in ["internal_error", "other_error"] {
        let mut glb = catalog(&f);
        glb.extend([
            ok(&json!([f["pool"]])),
            ok(&f["pool"]),
            empty(),
            ok(&json!([f["group"]])),
            ok(&f["group"]),
            response("202 Accepted", ""),
            empty(),
            internal(),
            ok(&f["parent"]),
            response(
                "500 Internal Server Error",
                json!({"code": code}).to_string(),
            ),
        ]);
        let (client, glb, regional) = servers(glb, vec![], true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let result = client
            .delete_pool(&ctx, DeletePoolInput::new("glb-1", "pool-1"))
            .await;
        if code == "internal_error" {
            assert!(result.unwrap().deleted);
        } else {
            let failure = result.unwrap_err();
            assert!(failure.error().has_identity(ErrorClass::NotSettled));
            assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
            assert!(!failure.partial().unwrap().deleted);
        }
        assert!(glb.heads()[8].starts_with(format!("DELETE {ROOT}{POOL} ").as_bytes()));
        assert_eq!(glb.requests(), 13);
        assert_eq!(regional.requests(), 0);
    }
}

#[tokio::test]
async fn catalog_refusals_never_reach_placement_or_mutation() {
    for case in [
        "duplicate-region",
        "disabled",
        "null-owner",
        "other-region-conflict",
    ] {
        let mut f = child_fixture();
        match case {
            "duplicate-region" => f["regions"][0]["id"] = json!("HAN_LAB"),
            "disabled" => f["packages"][0]["enabled"] = json!(false),
            "null-owner" => f["packages"][0]["vlbPackages"][0]["glb_package_id"] = Value::Null,
            _ => f["packages"][0]["vlbPackages"]
                .as_array_mut()
                .unwrap()
                .extend([
                    json!({"region":"hcm","vlb_package_id":"plan-2"}),
                    json!({"region":"hcm-3","vlb_package_id":"plan-3"}),
                ]),
        }
        let (client, glb, regional) = servers(catalog(&f), vec![], true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert!(failure.error().has_identity(ErrorClass::InvalidInput));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(glb.requests(), 3);
        assert_eq!(regional.requests(), 0);
    }
}

#[tokio::test]
async fn malformed_and_wrong_shape_catalog_responses_are_distinct() {
    use super::super::DecodeError;
    let f = child_fixture();
    for (body, reason) in [
        ("[", DecodeError::MalformedJson),
        ("{}", DecodeError::InvalidShape),
    ] {
        let (client, glb, _) = servers(
            vec![ok(&f["parent"]), response("200 OK", body)],
            vec![],
            true,
            TOKEN,
        );
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert_eq!(failure.error().detail::<DecodeError>(), Some(&reason));
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(glb.requests(), 2);
    }
}

#[tokio::test]
async fn malformed_acceptance_never_polls_an_ambiguous_id() {
    let mut f = child_fixture();
    f["pool_accept"]["ID"] = json!("different");
    let mut glb = catalog(&f);
    glb.extend([empty(), empty(), accepted(&f["pool_accept"])]);
    let (client, glb, _regional) = servers(glb, placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(failure.partial().is_none());
    assert_eq!(glb.requests(), 6);
}

#[tokio::test]
async fn unrestricted_and_noncanonical_listener_sources_fail_before_io() {
    let (client, glb, regional) = servers(vec![], vec![], true, TOKEN);
    let (ctx, _) = RequestContext::new();
    for cidrs in [
        vec!["192.0.2.1/24".into()],
        (0..256).map(|i| format!("{i}.0.0.0/8")).collect(),
    ] {
        let mut input = listener_input();
        input.allowed_cidrs = cidrs;
        let failure = client.create_listener(&ctx, input).await.unwrap_err();
        assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    }
    assert_eq!(glb.requests(), 0);
    assert_eq!(regional.requests(), 0);
}

#[tokio::test]
async fn encoded_credential_in_unknown_acceptance_field_prevents_confirmation() {
    let mut f = child_fixture();
    f["pool_accept"]["note"] = json!("prefix toy%2Dcredential suffix");
    let mut replies = catalog(&f);
    replies.extend([empty(), empty(), accepted(&f["pool_accept"])]);
    let (client, glb, _regional) = servers(replies, placement(&f), true, "toy-credential");
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(failure.partial().is_none());
    assert_eq!(glb.requests(), 6);
}

#[tokio::test]
async fn configured_credential_in_caller_fields_refuses_discovery() {
    let (client, glb, regional) = servers(vec![], vec![], false, "toy-credential");
    let (ctx, _) = RequestContext::new();
    let mut input = pool_input();
    input.member_name = "toy%2Dcredential".into();
    let failure = client.create_pool(&ctx, input).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(glb.requests(), 0);
    assert_eq!(regional.requests(), 0);
}

#[tokio::test]
async fn provider_rejections_keep_fixed_message_and_status_without_replay() {
    let f = child_fixture();
    for (status, code) in [
        ("400 Bad Request", 400),
        ("401 Unauthorized", 401),
        ("403 Forbidden", 403),
        ("429 Too Many Requests", 429),
    ] {
        let mut responses = catalog(&f);
        responses.extend([
            empty(),
            empty(),
            response(status, r#"{"message":"bad backend 10.1.2.3"}"#),
        ]);
        let (client, glb, _regional) = servers(responses, placement(&f), true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(code));
        assert_eq!(api.operation(), "globalloadbalancer.CreatePool");
        assert!(api.message() == "request rejected by the API");
        assert_eq!(glb.requests(), 6);
    }
}

#[tokio::test]
async fn delete_pool_cannot_confirm_absence_from_a_truncated_404() {
    let f = child_fixture();
    let mut responses = catalog(&f);
    responses.extend([
        ok(&json!([f["pool"]])),
        ok(&f["pool"]),
        empty(),
        ok(&json!([f["group"]])),
        ok(&f["group"]),
        response("202 Accepted", ""),
        empty(),
        Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\n{}".to_vec()),
    ]);
    let (client, glb, _regional) = servers(responses, vec![], true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let result = client
        .delete_pool(&ctx, DeletePoolInput::new("glb-1", "pool-1"))
        .await
        .unwrap_err();
    assert!(result.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(result.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(!result.partial().unwrap().deleted);
    assert_eq!(glb.requests(), 11);
}

#[tokio::test]
async fn placement_requires_a_private_ipv4_address_in_the_exact_subnet() {
    let mut f = child_fixture();
    f["server"]["data"]["internalInterfaces"][0]["fixedIp"] = json!("192.0.2.1");
    f["subnet"]["cidr"] = json!("192.0.2.0/24");
    let (client, glb, regional) = servers(catalog(&f), placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let result = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(result.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(result.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(glb.requests(), 3);
    assert_eq!(regional.requests(), 3);
}

#[tokio::test]
async fn catalog_rejects_noncanonical_and_duplicate_package_ownership_keys() {
    let f = child_fixture();
    for links in [
        r#"[{"region":"han-1","GLB_PACKAGE_ID":"other","vlb_package_id":"plan-1"}]"#,
        r#"[{"region":"han-1","glb_package_id":"pkg-1","glb_package_id":"other","vlb_package_id":"plan-1"}]"#,
    ] {
        let body = format!(r#"[{{"id":"pkg-1","enabled":true,"vlbPackages":{links}}}]"#);
        let replies = vec![
            ok(&f["parent"]),
            ok(&f["regions"]),
            response("200 OK", body),
        ];
        let (client, glb, regional) = servers(replies, placement(&f), true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert_eq!(
            failure.error().api_error().map(crate::ApiError::code),
            Some("InvalidResponse")
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(glb.requests(), 3);
        assert_eq!(regional.requests(), 0);
    }
}

#[tokio::test]
async fn go_simple_fold_flags_ambiguous_aliases_even_in_another_region() {
    let mut f = child_fixture();
    f["regions"].as_array_mut().unwrap().extend([
        json!({"id":"other-1","name":"É","status":"ACTIVE"}),
        json!({"id":"other-2","name":"é","status":"ACTIVE"}),
    ]);
    f["packages"][0]["vlbPackages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"region":"É","vlb_package_id":"plan-2"}));
    let (client, glb, regional) = servers(catalog(&f), placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(glb.requests(), 3);
    assert_eq!(regional.requests(), 0);
}

#[tokio::test]
async fn child_acknowledgements_and_collection_rows_require_strict_ids() {
    let mut f = child_fixture();
    f["pool_accept"]["globalPoolMembers"][0]["members"][0]["id"] = json!("row_1");
    let mut replies = catalog(&f);
    replies.extend([
        empty(),
        empty(),
        accepted(&f["pool_accept"]),
        ok(&f["parent"]),
    ]);
    replies.extend(chain(&f));
    replies.push(empty());
    let (client, glb, _regional) = servers(replies, placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    let partial = failure.partial().unwrap();
    assert_eq!(partial.pool.id, "pool-1");
    assert!(partial.pool_member.id.is_empty());
    assert_eq!(glb.requests(), 6);

    let mut replies = catalog(&f);
    replies.extend([
        ok(&json!([f["pool"]])),
        ok(&f["pool"]),
        empty(),
        ok(&json!([f["group"]])),
        ok(&f["group"]),
        response("202 Accepted", ""),
        ok(&json!([{"id":"other_1"}])),
        gone(),
        ok(&f["parent"]),
        gone(),
    ]);
    let (client, glb, _regional) = servers(replies, vec![], true, TOKEN);
    let failure = client
        .delete_pool(&ctx, DeletePoolInput::new("glb-1", "pool-1"))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(!failure.partial().unwrap().deleted);
    assert_eq!(glb.requests(), 10);
}

#[tokio::test]
async fn truncated_child_write_rejections_preserve_class_and_screen_status() {
    let f = child_fixture();
    for (token, expected) in [(TOKEN, Some(403)), ("403", None)] {
        let mut replies = catalog(&f);
        replies.extend([
            empty(),
            empty(),
            Response::RawClose(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\n{}".to_vec()),
        ]);
        let (client, glb, _regional) = servers(replies, placement(&f), true, token);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert!(failure.error().has_class(ErrorClass::PermissionDenied));
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.code(), "Forbidden");
        assert_eq!(api.status(), expected);
        assert!(api.message() == "request rejected by the API");
        assert!(!api.retryable());
        assert_eq!(glb.requests(), 6);
    }
}

#[tokio::test]
async fn canonical_guard_keys_cannot_hide_an_extra_region_or_interface() {
    for region_alias in [true, false] {
        let mut f = child_fixture();
        if region_alias {
            f["regions"].as_array_mut().unwrap().push(json!({
                "id": "other-1", "Name": "han-1", "status": "ACTIVE",
                "vserverEndpoint": "https://han-1.api.vngcloud.vn/vserver/vserver-gateway"
            }));
        } else {
            f["server"]["data"]["internalInterfaces"].as_array_mut().unwrap().push(json!({
                "ServerUuid": "server-1", "networkUuid": "vpc-1", "subnetUuid": "sub-1", "fixedIp": "10.1.2.4"
            }));
        }
        let mut replies = catalog(&f);
        replies.extend([
            empty(),
            empty(),
            accepted(&f["pool_accept"]),
            ok(&f["parent"]),
        ]);
        replies.extend(chain(&f));
        replies.push(empty());
        let (client, glb, regional) = servers(replies, placement(&f), true, TOKEN);
        let (ctx, _) = RequestContext::new();
        let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
        assert_eq!(
            failure.error().detail::<super::super::DecodeError>(),
            Some(&super::super::DecodeError::InvalidShape)
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(glb.requests(), if region_alias { 2 } else { 3 });
        assert_eq!(regional.requests(), usize::from(!region_alias));
    }
}

#[tokio::test]
async fn canonical_acknowledgement_keys_cannot_hide_a_backend_id() {
    let mut f = child_fixture();
    let group = f["pool_accept"]["globalPoolMembers"][0]
        .as_object_mut()
        .unwrap();
    group.remove("members");
    group.insert("Members".into(), json!([{"id":"row-2"}]));
    let mut replies = catalog(&f);
    replies.extend([
        empty(),
        empty(),
        accepted(&f["pool_accept"]),
        ok(&f["parent"]),
    ]);
    replies.extend(chain(&f));
    replies.push(empty());
    let (client, glb, _regional) = servers(replies, placement(&f), true, TOKEN);
    let (ctx, _) = RequestContext::new();
    let failure = client.create_pool(&ctx, pool_input()).await.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(glb.requests(), 6);
}
