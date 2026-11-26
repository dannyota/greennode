mod child;
mod writes;

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

const TOKEN: &str = "toy-glb-session-bearer";

fn response(status: &str, body: impl AsRef<[u8]>) -> Response {
    let body = body.as_ref();
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn client(server: &Server, bearer: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("unused-project".into())
            .with_static_token(Secret::new(bearer))
            .with_endpoint_overrides(EndpointOverrides {
                global_load_balancer: Some(format!("{}/glb-controller/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

macro_rules! fixture {
    ($name:literal) => {
        response(
            "200 OK",
            include_str!(concat!(
                "../../tests/fixtures/globalloadbalancer/",
                $name,
                ".json"
            )),
        )
    };
}

#[tokio::test]
async fn reads_decode_the_baseline_fixtures() {
    let server = Server::start(vec![
        fixture!("list_packages"),
        fixture!("list_regions"),
        fixture!("list_load_balancers"),
        fixture!("get_load_balancer"),
        fixture!("list_pools"),
        fixture!("list_listeners"),
        fixture!("get_listener"),
        fixture!("list_pool_members"),
        fixture!("get_pool_member"),
        fixture!("list_usage_histories"),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    let packages = client
        .list_packages(&ctx, ListPackagesInput {})
        .await
        .unwrap();
    assert_eq!(packages.items[0].id, "pkg-1");
    assert!(packages.items[0].enabled);
    let regions = client
        .list_regions(&ctx, ListRegionsInput {})
        .await
        .unwrap();
    assert_eq!(regions.items[0].id, "region-1");
    assert_eq!(regions.items[0].vserver_endpoint, "<internal-url>");
    let page = client
        .list_load_balancers(
            &ctx,
            ListLoadBalancersInput {
                offset: -1,
                limit: -1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items[0].id, "glb-1");
    let load_balancer = client
        .get_load_balancer(&ctx, GetLoadBalancerInput::new("glb-1"))
        .await
        .unwrap()
        .load_balancer;
    assert_eq!(load_balancer.id, "glb-1");
    assert!(!load_balancer.vips.is_empty());
    let pools = client
        .list_pools(&ctx, ListPoolsInput::new("glb-1"))
        .await
        .unwrap();
    assert_eq!(pools.items[0].id, "pool-1");
    assert!(pools.items[0].health.is_some());
    let listeners = client
        .list_listeners(&ctx, ListListenersInput::new("glb-1"))
        .await
        .unwrap();
    assert_eq!(listeners.items[0].id, "listener-1");
    let listener = client
        .get_listener(&ctx, GetListenerInput::new("glb-1", "listener-1"))
        .await
        .unwrap()
        .listener;
    assert_eq!(listener.id, "listener-1");
    assert!(!listener.allowed_cidrs.is_empty());
    let groups = client
        .list_pool_members(&ctx, ListPoolMembersInput::new("glb-1", "pool-1"))
        .await
        .unwrap();
    assert_eq!(groups.items[0].id, "member-1");
    let group = client
        .get_pool_member(&ctx, GetPoolMemberInput::new("glb-1", "pool-1", "member-1"))
        .await
        .unwrap()
        .pool_member;
    assert_eq!(group.id, "member-1");
    assert!(!group.members.is_empty());
    let usage = client
        .list_usage_histories(&ctx, ListUsageHistoriesInput::new("glb-1"))
        .await
        .unwrap();
    assert_eq!(usage.items.len(), 1);
    let heads = server.heads();
    assert_eq!(heads.len(), 10);
    assert!(
        heads[2].starts_with(
            b"GET /glb-controller/v1/global-load-balancers?limit=10000&name=&offset=0 "
        )
    );
    assert!(
        heads[9]
            .starts_with(b"GET /glb-controller/v1/global-load-balancers/glb-1/usage-histories ")
    );
}

#[tokio::test]
async fn null_and_empty_responses_have_go_zero_values() {
    let server = Server::start(vec![
        response("200 OK", ""),
        response("200 OK", "null"),
        response(
            "200 OK",
            r#"{"items":[null,{"id":null,"userId":null,"vips":null,"domains":[null]}],"offset":null}"#,
        ),
        response(
            "200 OK",
            r#"[{"enabled":null,"detail":null,"vlbPackages":null}]"#,
        ),
        response(
            "200 OK",
            r#"[{"health":{"timeout":null,"path":null},"stickySession":null,"deletedAt":null}]"#,
        ),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_regions(&ctx, ListRegionsInput {})
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        client
            .get_load_balancer(&ctx, GetLoadBalancerInput::new("glb-1"))
            .await
            .unwrap()
            .load_balancer,
        LoadBalancer::default()
    );
    let output = client
        .list_load_balancers(&ctx, ListLoadBalancersInput::default())
        .await
        .unwrap();
    assert_eq!(output.offset, 0);
    assert_eq!(output.items[0], LoadBalancer::default());
    assert_eq!(output.items[1].user_id, 0);
    assert!(output.items[1].vips.is_empty());
    assert_eq!(output.items[1].domains, vec![Domain::default()]);
    assert_eq!(
        client
            .list_packages(&ctx, ListPackagesInput {})
            .await
            .unwrap()
            .items,
        vec![Package::default()]
    );
    let pool = client
        .list_pools(&ctx, ListPoolsInput::new("glb-1"))
        .await
        .unwrap()
        .items
        .pop()
        .unwrap();
    assert_eq!(pool.health, Some(PoolHealthMonitor::default()));
    assert!(pool.sticky_session.is_none() && pool.deleted_at.is_none());
}

#[tokio::test]
async fn strict_decoding_distinguishes_syntax_from_wrong_shapes() {
    let samples = [
        ("{", DecodeError::MalformedJson),
        ("[]", DecodeError::InvalidShape),
        (r#"{"vips":[{"id":12,"Id":13}]}"#, DecodeError::InvalidShape),
        (r#"{"vips":[[]]}"#, DecodeError::InvalidShape),
        (r#"{"vips":[{"id":"5"}]}"#, DecodeError::InvalidShape),
    ];
    let server = Server::start(
        samples
            .iter()
            .map(|(body, _)| response("200 OK", body))
            .collect(),
    );
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    for (_, expected) in samples {
        let failure = client
            .get_load_balancer(&ctx, GetLoadBalancerInput::new("glb-1"))
            .await
            .unwrap_err();
        let (error, partial, dispatch) = failure.into_parts();
        assert!(partial.is_none());
        assert_eq!(dispatch, WriteDispatch::NotAWrite);
        assert_eq!(error.detail::<DecodeError>(), Some(&expected));
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), None);
        assert_eq!(
            api.message(),
            "globalloadbalancer.GetLoadBalancer: request failed: response failed to decode"
        );
        assert_eq!(api.operation(), "globalloadbalancer.GetLoadBalancer");
    }
}

#[tokio::test]
async fn required_and_invalid_path_ids_stop_before_a_request() {
    let server = Server::start(vec![]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    for (id, expected) in [
        ("", "requires PoolMemberID"),
        ("../child", "requires PoolMemberID to match"),
    ] {
        let error = client
            .get_pool_member(&ctx, GetPoolMemberInput::new("glb-1", "pool-1", id))
            .await
            .unwrap_err()
            .into_parts()
            .0;
        assert_eq!(error.class(), ErrorClass::InvalidInput);
        assert!(error.api_error().unwrap().message().contains(expected));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn numeric_credentials_are_refused_in_typed_values_and_redacted_in_dynamic_detail() {
    let bearer = "10000000";
    let server = Server::start(vec![
        response("200 OK", r#"{"items":[{"value":1e7}]}"#),
        response("200 OK", r#"[{"detail":{"large":1e7,"keep":23}}]"#),
    ]);
    let client = client(&server, bearer);
    let (ctx, _) = RequestContext::new();
    let error = client
        .list_usage_histories(&ctx, ListUsageHistoriesInput::new("glb-1"))
        .await
        .unwrap_err()
        .into_parts()
        .0;
    assert_eq!(
        error.detail::<DecodeError>(),
        Some(&DecodeError::ReflectedCredential)
    );
    assert!(!error.to_string().contains(bearer));
    let output = client
        .list_packages(&ctx, ListPackagesInput {})
        .await
        .unwrap();
    assert_eq!(output.items[0].detail["large"], "<redacted>");
    assert_eq!(output.items[0].detail["keep"].as_f64(), Some(23.0));
}

#[tokio::test]
async fn error_http_status_is_preserved_after_body_sanitization() {
    let body = format!(r#"{{"message":"{TOKEN}"}}"#);
    let server = Server::start(vec![
        response("404 Not Found", &body),
        response("403 Forbidden", &body),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    for (status, code, message) in [
        (404, "NotFound", "Not Found"),
        (403, "Forbidden", "Forbidden"),
    ] {
        let error = client
            .get_load_balancer(&ctx, GetLoadBalancerInput::new("glb-1"))
            .await
            .unwrap_err()
            .into_parts()
            .0;
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(status));
        assert_eq!(api.code(), code);
        assert!(!api.message().contains(TOKEN));
        assert_eq!(api.message(), message);
    }
}

#[tokio::test]
async fn percent_encoded_credentials_are_removed_from_strings_and_nested_keys() {
    let encoded = TOKEN.replace('-', "%252D");
    let headers = format!(r#"{{"headers":"https://example.invalid/?header={encoded}"}}"#);
    let packages = format!(
        r#"[{{"detail":[{{"{encoded}":"omit","name":"{encoded}","URL":"https://example.invalid/é%20x"}}]}}]"#
    );
    let server = Server::start(vec![
        response("200 OK", headers),
        response("200 OK", packages),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    let listener = client
        .get_listener(&ctx, GetListenerInput::new("glb-1", "listener-1"))
        .await
        .unwrap()
        .listener;
    assert!(listener.headers.as_deref() == Some("<redacted>"));
    let packages = client
        .list_packages(&ctx, ListPackagesInput {})
        .await
        .unwrap();
    let detail = &packages.items[0].detail[0];
    assert!(detail.get(&encoded).is_none());
    assert!(detail["name"] == "<redacted>");
    assert_eq!(detail["URL"], "https://example.invalid/é%20x");
}

#[tokio::test]
async fn dynamic_booleans_and_nulls_check_their_rendered_spellings() {
    let (ctx, _) = RequestContext::new();
    for (bearer, reflected, safe, expected) in [
        ("true", "true", "false", serde_json::json!(false)),
        ("false", "false", "true", serde_json::json!(true)),
        ("null", "null", "false", serde_json::json!(false)),
    ] {
        let body =
            format!(r#"[{{"enabled":{safe},"detail":{{"match":{reflected},"safe":{safe}}}}}]"#);
        let server = Server::start(vec![response("200 OK", body)]);
        let output = client(&server, bearer)
            .list_packages(&ctx, ListPackagesInput {})
            .await
            .unwrap();
        let detail = &output.items[0].detail;
        assert!(detail["match"] == "<redacted>");
        assert_eq!(detail["safe"], expected);
    }
}

#[tokio::test]
async fn invalid_unicode_is_replaced_before_shape_validation_and_redaction() {
    let body = b"[{\"name\":\"\xE2\x82x\",\"detail\":{\"\\uDC00\":\"\\uD800toy-glb-session-bearer\",\"pair\":\"\\uD834\\uDD1E\",\"literal\":\"\\\\uD800\"}}]";
    let server = Server::start(vec![
        response("200 OK", body),
        response("200 OK", br#"[{"detail":{"\uD800":1,"\uDC00":2}}]"#),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_packages(&ctx, ListPackagesInput {})
        .await
        .unwrap();
    assert_eq!(output.items[0].name, "��x");
    let detail = &output.items[0].detail;
    assert!(detail["�"] == "<redacted>");
    assert_eq!(detail["pair"], "𝄞");
    assert_eq!(detail["literal"], r"\uD800");
    let error = client
        .list_packages(&ctx, ListPackagesInput {})
        .await
        .unwrap_err()
        .into_parts()
        .0;
    assert_eq!(
        error.detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
}

#[tokio::test]
async fn go_integer_negative_zero_decodes_with_exact_signed_bounds() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"offset":-0,"limit":9223372036854775807,"total":-9223372036854775808,"items":[{"userId":-0}]}"#,
        ),
        response("200 OK", r#"{"total":9223372036854775808}"#),
        response("200 OK", r#"{"total":-0.0}"#),
    ]);
    let client = client(&server, TOKEN);
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_load_balancers(&ctx, ListLoadBalancersInput::default())
        .await
        .unwrap();
    assert_eq!(output.offset, 0);
    assert_eq!(output.limit, i64::MAX);
    assert_eq!(output.total, i64::MIN);
    assert_eq!(output.items[0].user_id, 0);
    for _ in 0..2 {
        let error = client
            .list_load_balancers(&ctx, ListLoadBalancersInput::default())
            .await
            .unwrap_err()
            .into_parts()
            .0;
        assert_eq!(
            error.detail::<DecodeError>(),
            Some(&DecodeError::InvalidShape)
        );
    }
}
