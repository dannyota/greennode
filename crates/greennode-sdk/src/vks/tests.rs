use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

const CLUSTERS: &str = include_str!("../../tests/fixtures/vks/cluster-list-schema.json");
const VERSIONS: &str = include_str!("../../tests/fixtures/vks/hcm-cluster-versions.json");
const QUOTA: &str = include_str!("../../tests/fixtures/vks/hcm-quota.json");
const BEARER: &str = "synthetic-vks-bearer-canary";

#[tokio::test]
async fn delete_provenance_rejects_explicit_default_port() {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_iam_user(crate::IamUserAuth::new(
                "root".into(),
                "user".into(),
                Secret::new("toy"),
            ))
            .with_endpoint_overrides(EndpointOverrides {
                vks: Some("https://vks.console.greennode.ai:443/vks-api/".into()),
                ..Default::default()
            }),
    )
    .unwrap();
    let (ctx, cancel) = RequestContext::new();
    cancel.cancel();
    let failure = Client::new(&config)
        .delete_cluster(&ctx, DeleteClusterInput::new("cluster-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        "greennode: invalid config: VKS writes require SDK IAM-user login and default endpoints"
    );
}

#[tokio::test]
async fn delete_preflight_redirect_preserves_status() {
    let server = Server::start(vec![Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Content-Length".into(), "0".into()),
            ("Location".into(), "/other".into()),
        ],
        body: Vec::new(),
    }]);
    let (ctx, _) = RequestContext::new();
    let failure = test_client(&server, "hcm-3", BEARER)
        .delete_cluster_inner(&ctx, DeleteClusterInput::new("cluster-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(302));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn delete_requires_iam_user_and_default_endpoints_before_any_request() {
    let server = Server::start(vec![]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_cluster(&ctx, DeleteClusterInput::new("cluster-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.connections(), 0);
}

#[test]
fn delete_output_uses_baseline_projection_name() {
    assert_eq!(
        serde_json::to_string(&DeleteClusterOutput { deleted: false }).unwrap(),
        r#"{"Deleted":false}"#
    );
}

#[tokio::test]
async fn cluster_settlement_refuses_reflected_active_state() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"id":"cluster-1","status":"ACTIVE"}"#,
    )]);
    let result = test_client(&server, "hcm-3", "ACTIVE")
        .cluster_state(
            &RequestContext::new().0,
            "cluster-1",
            &mut crate::wire::product::evidence::Evidence::new(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn delete_preflights_sends_once_and_confirms_absence() {
    let server = Server::start(vec![
        response("200 OK", r#"{"id":"cluster-1","status":"ACTIVE"}"#),
        response("202 Accepted", ""),
        response("404 Not Found", ""),
    ]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    let output = client
        .delete_cluster_inner(&ctx, DeleteClusterInput::new("cluster-1"))
        .await
        .unwrap();
    assert!(output.deleted);
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[0].starts_with(b"GET /vks-api/v1/clusters/cluster-1 "));
    assert!(heads[1].starts_with(b"DELETE /vks-api/v1/clusters/cluster-1 "));
    assert!(heads[2].starts_with(b"GET /vks-api/v1/clusters/cluster-1 "));
}

#[tokio::test]
async fn delete_failure_preserves_dispatch_and_partial_output() {
    let server = Server::start(vec![
        response("200 OK", r#"{"id":"cluster-1","status":"ACTIVE"}"#),
        response("503 Service Unavailable", ""),
    ]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_cluster_inner(&ctx, DeleteClusterInput::new("cluster-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.error().api_error().unwrap().status(), Some(503));
    let message = failure.error().detail::<String>().unwrap();
    assert!(message.contains("write outcome not confirmed"));
    assert!(message.contains("status 503"));
    assert!(message.contains("inspect inventory and billing"));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(!failure.partial().unwrap().deleted);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn delete_no_wait_returns_unconfirmed_and_preflight_failure_stops_write() {
    let server = Server::start(vec![
        response("200 OK", r#"{"id":"cluster-1","status":"ACTIVE"}"#),
        response("202 Accepted", ""),
        response("404 Not Found", ""),
    ]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteClusterInput::new("cluster-1");
    input.no_wait = true;
    assert!(
        !client
            .delete_cluster_inner(&ctx, input.clone())
            .await
            .unwrap()
            .deleted
    );
    let failure = client.delete_cluster_inner(&ctx, input).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotFound);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn test_client(server: &Server, region: &str, bearer: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region(region.into())
            .with_static_token(Secret::new(bearer))
            .with_endpoint_overrides(EndpointOverrides {
                vks: Some(format!("{}/vks-api/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn reads_one_zero_based_cluster_page_and_preserves_inventory() {
    let server = Server::start(vec![response("200 OK", CLUSTERS)]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    let result = client
        .list_clusters(&ctx, ListClustersInput { page: 1, size: 2 })
        .await
        .unwrap();
    assert_eq!(
        (
            result.page,
            result.page_size,
            result.total_page,
            result.total_item
        ),
        (1, 2, 3, 5)
    );
    assert_eq!(result.items.len(), 1);
    let cluster = &result.items[0];
    assert_eq!(cluster.id, "cls-1");
    assert_eq!(cluster.status, "UNKNOWN");
    assert_eq!(cluster.num_nodes, 3);
    assert!(cluster.enable_private_cluster);
    assert_eq!(server.requests(), 1);
    assert!(server.heads()[0].starts_with(b"GET /vks-api/v1/clusters?page=1&pageSize=2 "));
    assert!(String::from_utf8_lossy(&server.heads()[0]).contains(&format!("Bearer {BEARER}")));
}

#[tokio::test]
async fn catalog_and_quota_use_bare_regional_responses() {
    for (region, versions, quota) in [
        ("hcm-3", VERSIONS, QUOTA),
        (
            "han-1",
            include_str!("../../tests/fixtures/vks/han-cluster-versions.json"),
            include_str!("../../tests/fixtures/vks/han-quota.json"),
        ),
    ] {
        let server = Server::start(vec![
            response("200 OK", versions),
            response("200 OK", quota),
        ]);
        let client = test_client(&server, region, BEARER);
        let (ctx, _) = RequestContext::new();
        let versions = client
            .list_cluster_versions(&ctx, ListClusterVersionsInput)
            .await
            .unwrap();
        assert!(!versions.items.is_empty());
        let quota = client.get_quota(&ctx, GetQuotaInput).await.unwrap().quota;
        assert_eq!((quota.max_clusters, quota.num_clusters), (1, 2));
        let heads = server.heads();
        assert_eq!(server.requests(), 2);
        assert!(heads[0].starts_with(b"GET /vks-api/v1/cluster-versions "));
        assert!(heads[1].starts_with(b"GET /vks-api/v1/quota "));
    }
}

#[tokio::test]
async fn cluster_defaults_and_maximum_total_do_not_overflow() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../tests/fixtures/vks/hcm-clusters.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/vks/han-clusters.json"),
        ),
        response(
            "200 OK",
            r#"{"items":[{"id":"cls-1","name":null}],"total":9223372036854775807,"page":0,"pageSize":2}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    for region in ["hcm-3", "han-1"] {
        let actual = test_client(&server, region, BEARER)
            .list_clusters(&ctx, ListClustersInput::default())
            .await
            .unwrap();
        assert!(actual.items.is_empty());
        assert_eq!(actual.total_page, 0);
    }
    let actual = test_client(&server, "han-1", BEARER)
        .list_clusters(
            &ctx,
            ListClustersInput {
                page: 2147483647,
                size: 2147483647,
            },
        )
        .await
        .unwrap();
    assert_eq!(actual.total_page, 4611686018427387904);
    assert_eq!(actual.items[0].name, "");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vks-api/v1/clusters?page=0&pageSize=10 "));
    assert!(heads[2].starts_with(b"GET /vks-api/v1/clusters?page=2147483647&pageSize=2147483647 "));
}

#[tokio::test]
async fn invalid_inputs_and_unsupported_region_stop_before_requests() {
    let server = Server::start(vec![]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    for (page, size) in [(-1, 0), (0, -1), (2147483648, 0), (0, 2147483648)] {
        let failure = client
            .list_clusters(&ctx, ListClustersInput { page, size })
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
    }
    let invalid = test_client(&server, "other", BEARER);
    assert_eq!(
        invalid
            .get_quota(&ctx, GetQuotaInput)
            .await
            .unwrap_err()
            .error()
            .class(),
        ErrorClass::InvalidConfig,
    );
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn null_missing_and_wrongly_typed_wire_values_are_malformed() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"items":[null],"total":1,"page":0,"pageSize":10}"#,
        ),
        response("200 OK", r#"{"items":[],"total":0,"page":0,"pageSize":0}"#),
        response("200 OK", r#"[{"version":"synthetic","stage":"STABLE"}]"#),
        response(
            "200 OK",
            r#"[{"version":"synthetic","enable":"false","stage":"STABLE"}]"#,
        ),
        response(
            "200 OK",
            r#"{"maxClusters":0,"numClusters":null,"maxNodeGroupsPerCluster":0,"maxNodesPerNodeGroup":0}"#,
        ),
        response(
            "200 OK",
            r#"{"maxClusters":2147483648,"numClusters":0,"maxNodeGroupsPerCluster":0,"maxNodesPerNodeGroup":0}"#,
        ),
        response(
            "200 OK",
            r#"[{"version":"synthetic","enable":false,"stage":"STABLE","deprecatedAt":null}]"#,
        ),
    ]);
    let client = test_client(&server, "hcm-3", BEARER);
    for _ in 0..2 {
        assert_malformed(
            client
                .list_clusters(&ctx, ListClustersInput::default())
                .await
                .unwrap_err(),
        );
    }
    for _ in 0..2 {
        assert_malformed(
            client
                .list_cluster_versions(&ctx, ListClusterVersionsInput)
                .await
                .unwrap_err(),
        );
    }
    for _ in 0..2 {
        assert_malformed(client.get_quota(&ctx, GetQuotaInput).await.unwrap_err());
    }
    let versions = client
        .list_cluster_versions(&ctx, ListClusterVersionsInput)
        .await
        .unwrap();
    assert!(!versions.items[0].enable);
    assert!(versions.items[0].deprecated_at.is_empty());
}

fn assert_malformed<T>(failure: Failure<T>) {
    assert_eq!(failure.error().class(), ErrorClass::Api);
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.message(), "malformed response; body withheld");
    assert!(failure.partial().is_none());
}

#[tokio::test]
async fn reflected_bearers_in_strings_are_redacted_before_projection() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"[{"version":"prefix synthetic-vks-%62earer-canary","enable":false,"stage":"STABLE"}]"#,
        ),
        response(
            "200 OK",
            r#"{"items":[{"name":"synthetic-vks-%62earer-canary","description":"resource-canary","numNodes":3}],"total":1,"page":0,"pageSize":10}"#,
        ),
        response(
            "200 OK",
            r#"{"maxClusters":9,"numClusters":123,"maxNodeGroupsPerCluster":3,"maxNodesPerNodeGroup":2}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = test_client(&server, "hcm-3", BEARER);
    let versions = client
        .list_cluster_versions(&ctx, ListClusterVersionsInput)
        .await
        .unwrap();
    assert!(versions.items[0].version == "<redacted>");
    assert_eq!(versions.items[0].stage, "STABLE");
    assert!(!versions.items[0].enable);
    let clusters = client
        .list_clusters(&ctx, ListClustersInput::default())
        .await
        .unwrap();
    assert!(clusters.items[0].name == "<redacted>");
    assert_eq!(clusters.items[0].description, "resource-canary");
    assert_eq!(clusters.items[0].num_nodes, 3);
    assert_malformed(
        test_client(&server, "hcm-3", "+123")
            .get_quota(&ctx, GetQuotaInput)
            .await
            .unwrap_err(),
    );
}

#[tokio::test]
async fn quota_keeps_status_and_retryability_without_provider_text() {
    let cases = [
        ("202 Accepted", 202, "", ErrorClass::Api, false),
        ("400 Bad Request", 400, "BadRequest", ErrorClass::Api, false),
        (
            "401 Unauthorized",
            401,
            "Unauthorized",
            ErrorClass::Auth,
            false,
        ),
        (
            "403 Forbidden",
            403,
            "Forbidden",
            ErrorClass::PermissionDenied,
            false,
        ),
        (
            "404 Not Found",
            404,
            "NotFound",
            ErrorClass::NotFound,
            false,
        ),
        (
            "429 Too Many Requests",
            429,
            "Throttled",
            ErrorClass::RateLimited,
            true,
        ),
        (
            "500 Internal Server Error",
            500,
            "ServerError",
            ErrorClass::Api,
            false,
        ),
        ("502 Bad Gateway", 502, "ServerError", ErrorClass::Api, true),
        (
            "503 Service Unavailable",
            503,
            "ServerError",
            ErrorClass::Api,
            true,
        ),
        (
            "504 Gateway Timeout",
            504,
            "ServerError",
            ErrorClass::Api,
            true,
        ),
    ];
    let mut tasks = tokio::task::JoinSet::new();
    for (status, number, code, class, retryable) in cases {
        tasks.spawn(async move {
            let attempts = if retryable { 4 } else { 1 };
            let server = Server::start((0..attempts).map(|_| {
                response(status, r#"{"code":"synthetic-provider-marker","error":{"message":"synthetic-provider-marker"}}"#)
            }).collect());
            let (ctx, _) = RequestContext::new();
            let failure = test_client(&server, "hcm-3", BEARER).get_quota(&ctx, GetQuotaInput).await.unwrap_err();
            assert_eq!(failure.error().class(), class);
            let api = failure.error().api_error().unwrap();
            assert_eq!(api.status(), Some(number));
            assert_eq!(api.code(), code);
            assert_eq!(api.message(), "request failed; response withheld");
            assert_eq!(api.operation(), "vks.GetQuota");
            assert_eq!(api.retryable(), retryable);
            assert_eq!(server.requests(), attempts);
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

#[tokio::test]
async fn cancellation_keeps_its_class_and_skips_network() {
    let server = Server::start(vec![]);
    let (ctx, cancel) = RequestContext::new();
    cancel.cancel();
    let error = test_client(&server, "hcm-3", BEARER)
        .get_quota(&ctx, GetQuotaInput)
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::Cancelled);
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn cross_origin_redirect_is_withheld_and_never_followed() {
    let other = Server::start(vec![]);
    let server = Server::start(vec![Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Content-Length".into(), "0".into()),
            (
                "Location".into(),
                format!("{}/synthetic-location-marker", other.origin),
            ),
        ],
        body: Vec::new(),
    }]);
    let (ctx, _) = RequestContext::new();
    let failure = test_client(&server, "hcm-3", BEARER)
        .get_quota(&ctx, GetQuotaInput)
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), None);
    assert_eq!(api.message(), "request failed; response withheld");
    assert_eq!(server.requests(), 1);
    assert_eq!(other.connections(), 0);
}

#[test]
fn sign_in_failures_keep_only_the_baseline_safe_details() {
    let provider_marker = "synthetic-sign-in-provider-marker";
    let exchange = Error::new(ErrorClass::Auth).with_service_login(
        crate::ServiceAccountLoginError::new(provider_marker, Some(401)),
    );
    let error = safe_error(super::QUOTA, &exchange);
    assert!(error.has_identity(ErrorClass::Auth));
    assert!(error.service_account_login_error().is_none());
    let api = error.api_error().unwrap();
    assert_eq!(api.operation(), super::QUOTA);
    assert_eq!(api.status(), None);
    assert!(api.code().is_empty());
    assert!(api.message() == "request failed; response withheld");

    let login =
        Error::new(ErrorClass::Auth).with_login(LoginError::new(provider_marker, Some(200), true));
    let error = safe_error(super::VERSIONS, &login);
    let login = error.login_error().unwrap();
    assert_eq!(login.status(), Some(200));
    assert!(login.captcha_suspected());
    let api = error.api_error().unwrap();
    assert_eq!(api.code(), "LoginFailed");
    assert_eq!(api.operation(), "");
    assert_eq!(api.status(), None);
    assert!(
        api.message()
            == "vks.ListClusterVersions: authentication failed; details withheld (status 200); a captcha may be required"
    );
}

#[tokio::test]
async fn incomplete_responses_drop_status_except_for_baseline_overflows() {
    let oversized = |status: &str| Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), ((8 << 20) + 1).to_string())],
        body: vec![b' '; (8 << 20) + 1],
    };
    let server = Server::start(vec![
        Response::RawClose(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}".to_vec()),
        response("302 Found", ""),
        oversized("200 OK"),
        oversized("404 Not Found"),
        oversized("302 Found"),
    ]);
    let client = test_client(&server, "hcm-3", BEARER);
    let (ctx, _) = RequestContext::new();
    for (status, code, message) in [
        (None, "", "request failed; response withheld"),
        (Some(302), "", "request failed; response withheld"),
        (Some(200), "", "malformed response; body withheld"),
        (Some(404), "NotFound", "request failed; response withheld"),
        (None, "", "request failed; response withheld"),
    ] {
        let failure = client.get_quota(&ctx, GetQuotaInput).await.unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), status);
        assert_eq!(api.code(), code);
        assert_eq!(api.message(), message);
        assert_eq!(api.operation(), super::QUOTA);
    }
    assert_eq!(server.requests(), 5);
}
