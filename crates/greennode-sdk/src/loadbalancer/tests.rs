use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, Secret};

const CANARY: &str = "toy-secret";

fn response(status: &str, raw: impl AsRef<[u8]>) -> Response {
    let raw = raw.as_ref();
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.to_vec(),
    }
}

fn client(server: &Server, token: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new(token))
            .with_endpoint_overrides(EndpointOverrides {
                vlb: Some(format!("{}/regional/", server.origin)),
                portal: Some(format!("{}/billing/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

fn pool_members() -> ListPoolMembersInput {
    ListPoolMembersInput {
        load_balancer_id: "lb-1".into(),
        pool_id: "pool-1".into(),
    }
}

#[tokio::test]
async fn child_list_distinguishes_empty_missing_and_wrong_shape() {
    let server = Server::start(
        [
            r#"{"data":[]}"#,
            r#"{}"#,
            r#"{"data":null}"#,
            r#"{"data":{}}"#,
            "{",
            r#"{"data":[],"Data":[]}"#,
        ]
        .into_iter()
        .map(|raw| response("200 OK", raw))
        .collect(),
    );
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_pool_members(&ctx, pool_members())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    for detail in [
        None,
        None,
        Some(DecodeError::InvalidShape),
        Some(DecodeError::MalformedJson),
        Some(DecodeError::InvalidShape),
    ] {
        let error = client
            .list_pool_members(&ctx, pool_members())
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(api.status(), None);
        assert_eq!(api.operation(), "loadbalancer.ListPoolMembers");
        let message = if detail.is_some() {
            "loadbalancer.ListPoolMembers: request failed: response failed to decode"
        } else {
            "list response had no data"
        };
        assert_eq!(api.message(), message);
        assert_eq!(error.error().detail::<DecodeError>().copied(), detail);
    }
    assert!(server.heads().iter().all(|head| {
        head.starts_with(b"GET /regional/v2/proj-1/loadBalancers/lb-1/pools/pool-1/members ")
    }));
}

#[tokio::test]
async fn credentials_are_screened_in_inputs_and_projected_output() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"data":[{"uuid":"member_1","name":" toy%2Dsecret ","weight":-0}]}"#,
    )]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    for id in ["../lb", "lb_1", CANARY] {
        let input = GetLoadBalancerInput {
            load_balancer_id: id.into(),
        };
        let error = client.get_load_balancer(&ctx, input).await.unwrap_err();
        assert!(error.error().has_class(ErrorClass::InvalidInput));
    }
    let error = client
        .list_packages(
            &ctx,
            ListPackagesInput {
                zone_id: "toy%2Dsecret".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(server.requests(), 0);

    let list = client
        .list_pool_members(&ctx, pool_members())
        .await
        .unwrap();
    assert_eq!(list.items[0].uuid, "member_1");
    assert_eq!(list.items[0].name, "<redacted>");
    assert_eq!(list.items[0].weight, 0);
}

#[tokio::test]
async fn models_handle_go_strings_and_block_number_pseudo_objects() {
    let server = Server::start(vec![
        response(
            "200 OK",
            b"{\"data\":{\"name\":\"a\xff\\ud800\",\"zoneId\":\"zone_api\",\"zone\":{\"name\":4}}}",
        ),
        response(
            "200 OK",
            r#"{"data":{"minSize":{"$serde_json::private::Number":"1"}}}"#,
        ),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let input = GetLoadBalancerInput {
        load_balancer_id: "lb-1".into(),
    };
    let result = client.get_load_balancer(&ctx, input.clone()).await.unwrap();
    assert_eq!(result.load_balancer.name, "a\u{fffd}\u{fffd}");
    assert_eq!(result.load_balancer.zone_id, "zone_api");
    let error = client.get_load_balancer(&ctx, input).await.unwrap_err();
    assert_eq!(
        error.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
}

#[tokio::test]
async fn quote_posts_only_priced_fields_to_portal() {
    let quote = r#"{"optimumPrice":12500,"propertiesPrice":[]}"#;
    let portal = Server::start(vec![response("200 OK", quote), response("200 OK", quote)]);
    let client = client(&portal, CANARY);
    let (ctx, _) = RequestContext::new();
    let create = CreateLoadBalancerInput {
        name: "demo".into(),
        package_id: "package_1".into(),
        r#type: "Layer 4".into(),
        scheme: "Internal".into(),
        subnet_id: "subnet-1".into(),
        zone_id: "zone_1".into(),
        max_price: 1.0,
        no_wait: true,
    };
    let blocked = CreateLoadBalancerInput {
        zone_id: "toy%2Dsecret".into(),
        ..create.clone()
    };
    assert!(
        client
            .quote_create_load_balancer(&ctx, blocked)
            .await
            .unwrap_err()
            .error()
            .has_class(ErrorClass::InvalidInput)
    );
    assert_eq!(portal.requests(), 0);
    client
        .quote_create_load_balancer(&ctx, create)
        .await
        .unwrap();
    client
        .quote_resize_load_balancer(
            &ctx,
            ResizeLoadBalancerInput {
                load_balancer_id: "lb-1".into(),
                package_id: "package_1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(portal.requests(), 2);
    assert!(
        portal
            .heads()
            .iter()
            .all(|head| head.starts_with(b"POST /billing/v1/price "))
    );
    let expected = [
        serde_json::json!({"resourceType":"load-balancer","action":"create","resourceInfo":{"packageId":"package_1","zoneId":"zone_1","isBuyMorePoc":false}}),
        serde_json::json!({"resourceType":"load-balancer","action":"resize","resourceInfo":{"loadBalancerId":"lb-1","packageId":"package_1"}}),
    ];
    for (raw, expected) in portal.bodies().iter().zip(expected) {
        let body: serde_json::Value = serde_json::from_slice(raw).unwrap();
        assert!(body == expected, "unexpected quote request shape");
    }
}
