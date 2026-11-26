use super::*;
use crate::testing::{Product, Response, Server, with_product_loopback};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret};

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn options() -> ConfigOptions {
    ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("toy-secret"))
}

#[tokio::test]
async fn nat_detail_discovers_zone_and_checks_returned_identity() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/get_nat_instance.json"),
        ),
        response("200 OK", r#"{"success":true,"data":{"uuid":"other-nat"}}"#),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let nat = client
        .get_nat_instance(&ctx, GetNatInstanceInput::new("nat-1"))
        .await
        .unwrap()
        .nat;
    assert_eq!(nat.vpc_id, "vpc-1");
    let mut input = GetNatInstanceInput::new("nat-1");
    input.zone_id = "zone-1".into();
    let failure = client
        .get_nat_instance(&ctx, input.clone())
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(200), "InvalidResponse", "NAT response withheld")
    );
    input.nat_id = "toy-secret".into();
    let failure = client.get_nat_instance(&ctx, input).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/regions "));
    assert!(heads[1].starts_with(b"GET /vnetwork/v1/zone-1/proj-1/nats/nat-1 "));
}

#[tokio::test]
async fn inbound_rules_accept_hcm_type_ids_and_refuse_incomplete_responses() {
    let body = include_str!("../../tests/fixtures/network/list_nat_inbound_rules_hcm.json");
    let incomplete = body.replace("\"code\":200", "\"totalPage\":2");
    let server = Server::start(vec![
        response("200 OK", body),
        response("200 OK", &incomplete),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let mut input = ListNatInboundRulesInput::new("nat-1");
    input.zone_id = "zone-1".into();
    let output = client
        .list_nat_inbound_rules(&ctx, input.clone())
        .await
        .unwrap();
    assert!(output.items[0].id.contains('_'));
    assert_eq!(output.items[0].port_range_min, None);
    assert_eq!(output.items[0].port_range_max, Some(0));
    assert_eq!(output.items[0].description, "<redacted>");
    let error = client
        .list_nat_inbound_rules(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert!(
        server.heads()[0]
            .starts_with(b"GET /vnetwork/v1/zone-1/proj-1/nats/nat-1/sec-groups/rules ")
    );
}

#[tokio::test]
async fn bandwidth_uses_fixed_origin_and_discovers_its_own_project() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"proj-1","region":"hcm-3"}]}"#,
        ),
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"proj-2","region":"hcm-3"}]}"#,
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_bandwidth_packages.json"),
        ),
        response("401 Unauthorized", r#"{"message":"toy%2Dsecret"}"#),
    ]);
    let config = Config::new(with_product_loopback(Product::Vserver, options(), &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let session = config.session();
    assert_eq!(session.require_project_id(&ctx).await.unwrap(), "proj-1");
    assert!(session.configured_project_id().is_none());
    let client = Client::new(&config);
    let output = client
        .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
        .await
        .unwrap();
    assert_eq!(output.items[0].bandwidth, 100);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-2".into()),
        &server,
    ))
    .unwrap();
    assert_eq!(config.session().configured_project_id(), Some("proj-2"));
    let failure = Client::new(&config)
        .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    let heads = server.heads();
    assert!(heads[1].starts_with(b"GET /vserver/iam-vserver-gateway/v1/projects "));
    assert_eq!(heads.len(), 4);
    assert!(
        heads[2].starts_with(
            b"GET /vserver/bandwidth-api/v1/projects/proj-2/bandwidth/package-config "
        )
    );

    let untrusted = Config::new(options().with_endpoint_overrides(EndpointOverrides {
        vserver: Some(format!("{}/", server.origin)),
        ..EndpointOverrides::default()
    }))
    .unwrap();
    let client = Client::new(&untrusted);
    assert_eq!(
        client.bandwidth_base().unwrap().to_string(),
        "https://hcm-3.console.greennode.ai/"
    );
}
