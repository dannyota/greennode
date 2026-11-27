mod endpoint_quotes;
mod refusals;
mod review;
mod variants;
mod zones;

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

fn assert_invalid<T>(result: Result<T, crate::Failure<T>>) {
    let Err(failure) = result else {
        panic!("expected failure")
    };
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
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
async fn nat_retained_literal_credential_is_invalid_response() {
    let body = include_str!("../../tests/fixtures/network/list_nat_inbound_rules_hcm.json")
        .replace("toy%2Dsecret", "toy-secret");
    let server = Server::start(vec![response("200 OK", &body)]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let mut input = ListNatInboundRulesInput::new("nat-1");
    input.zone_id = "zone-1".into();
    let failure = Client::new(&config)
        .list_nat_inbound_rules(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
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
async fn returned_identifiers_reject_embedded_encoded_credentials() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":[{"uuid":"abceHl6def","name":"offer","type":"x","description":"ok","bandwidth":100}]}"#,
        ),
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"abceHl6def","region":"hcm-3"}]}"#,
        ),
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"abceHl6def","protocol":"tcp","etherType":"IPv4","direction":"ingress","portRangeMin":1,"portRangeMax":2,"isSystem":false}]}"#,
        ),
    ]);
    let opts = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("xyz"));
    let (ctx, _) = RequestContext::new();
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        opts.clone().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    assert_invalid(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await,
    );
    assert_eq!(server.heads().len(), 1);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        opts.clone(),
        &server,
    ))
    .unwrap();
    assert_invalid(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await,
    );
    assert_eq!(server.heads().len(), 2);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        opts.with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let mut input = ListNatInboundRulesInput::new("nat-1");
    input.zone_id = "zone-1".into();
    assert_invalid(
        Client::new(&config)
            .list_nat_inbound_rules(&ctx, input)
            .await,
    );
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn returned_identifiers_reject_hex_credentials() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":[{"uuid":"prefix746f6b656e76616c7565suffix","name":"offer","type":"x","description":"ok","bandwidth":100}]}"#,
        ),
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"prefix746f6b656e76616c7565suffix","region":"hcm-3"}]}"#,
        ),
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"prefix746f6b656e76616c7565suffix","protocol":"tcp","etherType":"IPv4","direction":"ingress","portRangeMin":1,"portRangeMax":2,"isSystem":false}]}"#,
        ),
    ]);
    let opts = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("tokenvalue"));
    let (ctx, _) = RequestContext::new();
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        opts.clone().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    assert_invalid(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await,
    );
    assert_eq!(server.heads().len(), 1);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        opts.clone(),
        &server,
    ))
    .unwrap();
    assert_invalid(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await,
    );
    assert_eq!(server.heads().len(), 2);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        opts.with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let mut input = ListNatInboundRulesInput::new("nat-1");
    input.zone_id = "zone-1".into();
    assert_invalid(
        Client::new(&config)
            .list_nat_inbound_rules(&ctx, input)
            .await,
    );
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn bandwidth_refuses_a_lone_case_variant_key() {
    let body = r#"{"Success":false,"data":[{"uuid":"pkg-1","name":"offer","type":"x","description":"ok","bandwidth":100}]}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bandwidth_accepts_credential_words_in_description() {
    let body = r#"{"data":[{"uuid":"pkg-1","name":"offer","type":"x","description":"client secret","bandwidth":100}]}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn bandwidth_accepts_period_in_package_id() {
    let body = r#"{"data":[{"uuid":"pkg.1","name":"offer","type":"x","description":"ok","bandwidth":100}]}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let output = Client::new(&config)
        .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
        .await
        .unwrap();
    assert_eq!(output.items[0].id, "pkg.1");
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

#[tokio::test]
async fn metadata_discovery_records_its_bearer_in_caller_evidence() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":123}]}"#,
    )]);
    struct Provider;
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>>
        {
            Box::pin(async { Ok(crate::Token::new(Secret::new("discovery-only-toy"), None)) })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_credentials_provider(std::sync::Arc::new(Provider)),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let mut evidence = crate::wire::product::evidence::Evidence::new();
    let project = config
        .session()
        .require_project_with_evidence(&ctx, &mut evidence)
        .await
        .unwrap();
    assert_eq!(project.user_id, 123);
    assert!(evidence.id_reflects("discovery-only-toy"));
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn endpoint_tags_use_numeric_project_metadata_on_the_vnetwork_origin() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":123}]}"#,
    )]);
    let endpoint = Server::start(vec![response(
        "200 OK",
        include_str!("../../tests/fixtures/network/list_endpoint_tags.json"),
    )]);
    let options = options().with_project_id("proj-1".into());
    let options = with_product_loopback(Product::Vserver, options, &project);
    let options = with_product_loopback(Product::Vnetwork, options, &endpoint);
    let config = Config::new(options).unwrap();
    let (ctx, _) = RequestContext::new();
    let client = Client::new(&config);
    let failure = client
        .list_endpoint_tags(&ctx, ListEndpointTagsInput::new("toy-secret"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert!(project.heads().is_empty() && endpoint.heads().is_empty());
    let tags = client
        .list_endpoint_tags(&ctx, ListEndpointTagsInput::new("endpoint-1"))
        .await
        .unwrap();
    assert_eq!(tags.items[0].key, "env");
    assert!(project.heads()[0].starts_with(b"GET /v1/projects "));
    let headers = String::from_utf8_lossy(&endpoint.heads()[0]).to_ascii_lowercase();
    assert!(headers.starts_with("get /vnetwork/v1/proj-1/tags?resourceuuid=endpoint-1 "));
    assert!(headers.contains("\r\nportal-user-id: 123\r\n"));
}

#[tokio::test]
async fn server_lookup_decodes_distinct_shapes_into_the_shared_model() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_servers_by_security_group.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_servers_by_subnet.json"),
        ),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let client = Client::new(&config);
    let by_group = client
        .list_servers_by_security_group(&ctx, ListServersBySecurityGroupInput::new("group-1"))
        .await
        .unwrap();
    let by_subnet = client
        .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
        .await
        .unwrap();
    let first: &crate::compute::Server = &by_group.items[0];
    assert_eq!(first.uuid, by_subnet.items[0].uuid);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /v2/proj-1/secgroups/group-1/servers "));
    assert!(heads[1].starts_with(b"GET /v2/proj-1/servers/subnets/subnet-1 "));
}

#[tokio::test]
async fn nat_catalogs_send_distinct_queries_and_withhold_reflected_success() {
    let package = include_str!("../../tests/fixtures/network/list_nat_packages.json");
    let reflected = package.replace("Standard", "toy%2Dsecret");
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response("200 OK", package),
        response("200 OK", &reflected),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_nat_instances.json"),
        ),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let zones = client
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: "zone-1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(zones.items[0].uuid, "az-1");
    let mut input = ListNatPackagesInput::new("az-1");
    input.zone_id = "zone-1".into();
    let packages = client.list_nat_packages(&ctx, input.clone()).await.unwrap();
    assert_eq!(packages.items[0].price.optimum_price, 100.0);
    let failure = client.list_nat_packages(&ctx, input).await.unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    let list = client
        .list_nat_instances(
            &ctx,
            ListNatInstancesInput {
                zone_id: "zone-1".into(),
                page: 2,
                size: 5,
            },
        )
        .await
        .unwrap();
    assert!(!list.items.is_empty());
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/zone-1/proj-1/nats/zones?params="));
    let package_head = String::from_utf8_lossy(&heads[1]);
    assert!(package_head.starts_with("GET /vnetwork/v1/zone-1/proj-1/nats/nat-package?params="));
    assert!(package_head.contains("&zoneUuid=az-1 "));
    assert!(heads[3].starts_with(b"GET /vnetwork/v1/zone-1/proj-1/nats?params="));
}

#[tokio::test]
async fn prices_preflight_and_quote_three_public_ip_kinds() {
    let body = include_str!("../../tests/fixtures/pricing/discovery_quote_public_vm.json");
    let server = Server::start(vec![
        response("200 OK", body),
        response("200 OK", body),
        response("200 OK", body),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Portal,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_network_prices(
                &ctx,
                ListNetworkPricesInput {
                    max_requests: 2,
                    allow_partial: false
                }
            )
            .await
            .is_err()
    );
    assert!(server.heads().is_empty());
    let output = client
        .list_network_prices(&ctx, ListNetworkPricesInput::default())
        .await
        .unwrap();
    assert_eq!(output.metadata.requests, 3);
    assert_eq!(
        output
            .items
            .iter()
            .filter(|row| row.price.status == "quoted")
            .count(),
        3
    );
    assert_eq!(output.items.len(), 6);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"POST /v1/price "))
    );
}

#[tokio::test]
async fn nat_quote_checks_catalog_placement_before_posting_to_vnetwork() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/list_nat_packages.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/nat_quote_vpcs.json"),
        ),
        response(
            "200 OK",
            include_str!("../../tests/fixtures/network/quote_create_nat.json"),
        ),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let mut input =
        QuoteCreateNatInstanceInput::new("toy-secret", "zone-1", "az-1", "package-1", "vpc-1");
    assert!(
        client
            .quote_create_nat_instance(&ctx, input.clone())
            .await
            .is_err()
    );
    assert!(server.heads().is_empty());
    input.name = "edge".into();
    let quote = client.quote_create_nat_instance(&ctx, input).await.unwrap();
    assert_eq!(quote.monthly_price, 712_400.0);
    let heads = server.heads();
    assert_eq!(heads.len(), 5);
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/zone-1/proj-1/nats/v3-whitelist "));
    assert!(heads[4].starts_with(b"POST /vnetwork/billing/v1/zone-1/proj-1/price "));
}
