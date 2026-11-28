use super::*;

#[tokio::test]
async fn server_rejects_short_credential_in_nested_identifiers() {
    let body = r#"[{"uuid":"server-1","internalInterfaces":[{"networkUuid":"q7"}]}]"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("q7"))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn server_inventory_accepts_eight_mib_boundary() {
    let suffix = r#"{"data":[]}"#;
    let body = format!("{}{suffix}", " ".repeat((8 << 20) - suffix.len()));
    let server = Server::start(vec![response("200 OK", &body)]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_servers_by_security_group(&ctx, ListServersBySecurityGroupInput::new("sg-1"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn nat_inventory_checks_zone_before_pagination_without_echoing_the_id() {
    let server = Server::start(Vec::new());
    let config = Config::new(with_product_loopback(Product::Vnetwork, options(), &server)).unwrap();
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let diagnostic = "vngcloud: invalid input: network.ListNATInstances requires ZoneID to match ^[A-Za-z0-9-]+$";
    for zone_id in ["not/valid", "bad/toy-secret"] {
        let input = ListNatInstancesInput {
            zone_id: zone_id.into(),
            page: -1,
            ..Default::default()
        };
        let failure = client.list_nat_instances(&ctx, input).await.unwrap_err();
        let error = failure.error();
        assert_eq!(error.class(), ErrorClass::InvalidInput);
        assert!(error.api_error().is_none());
        assert!(error.detail::<String>().map(String::as_str) == Some(diagnostic));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn nat_inventory_preserves_safe_peer_error_code() {
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"code":"ExampleFailure","message":"missing"}"#,
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListNatInstancesInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    let failure = Client::new(&config)
        .list_nat_instances(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "ExampleFailure"
    );
}

#[tokio::test]
async fn nat_inventory_truncated_not_found_is_body_read_failure() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 100\r\n\r\n{}".to_vec(),
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListNatInstancesInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    let failure = Client::new(&config)
        .list_nat_instances(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Api);
    let api = failure.error().api_error().unwrap();
    assert_eq!((api.status(), api.code(), api.message()), (None, "", ""));
}

#[tokio::test]
async fn server_rejects_public_ip_derived_from_credential() {
    let body = r#"[{"uuid":"server-1","internalInterfaces":[{"floatingIp":"2001:4860:4860:0:0:0:0:8888"}]}]"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("2001:4860:4860::8888"))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
        .await;
    assert!(result.is_err());
    assert_eq!(
        result.err().unwrap().error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn network_prices_recheck_earlier_rounded_amount_against_later_token() {
    struct Provider(std::sync::atomic::AtomicUsize);
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::Token::new(
                    Secret::new(if n < 2 { "first-token" } else { "12.34" }),
                    None,
                ))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let body = include_str!("../../../tests/fixtures/pricing/discovery_quote_public_vm.json")
        .replace("120000", "12.339");
    let last = include_str!("../../../tests/fixtures/pricing/discovery_quote_public_vm.json");
    let server = Server::start(vec![
        response("200 OK", &body),
        response("200 OK", &body),
        response("200 OK", last),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(std::sync::Arc::new(Provider(
            std::sync::atomic::AtomicUsize::new(0),
        )));
    let config = Config::new(with_product_loopback(Product::Portal, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .list_network_prices(&ctx, ListNetworkPricesInput::default())
        .await;
    let failure = result.unwrap_err();
    assert!(failure.partial().is_none());
    let api = failure.error().api_error().unwrap();
    assert_eq!((api.status(), api.code()), (Some(200), "InvalidResponse"));
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn endpoint_tags_reject_hex_encoded_bearer_in_user_header() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":61}]}"#,
    )]);
    let endpoint = Server::start(vec![response("200 OK", r#"{"data":[]}"#)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_static_token(Secret::new("a"));
    let options = with_product_loopback(Product::Vserver, options, &project);
    let options = with_product_loopback(Product::Vnetwork, options, &endpoint);
    let config = Config::new(options).unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_endpoint_tags(&ctx, ListEndpointTagsInput::new("endpoint-1"))
            .await
            .is_err()
    );
    assert!(endpoint.heads().is_empty());
}

#[test]
fn endpoint_lone_case_variant_is_ignored_and_collisions_are_rejected() {
    let item: Endpoint = serde_json::from_str(r#"{"UUID":"endpoint-1"}"#).unwrap();
    assert!(item.uuid.is_empty());
    assert!(
        serde_json::from_str::<Endpoint>(r#"{"uuid":"endpoint-1","UUID":"endpoint-2"}"#).is_err()
    );
}

#[tokio::test]
async fn unpriced_quote_errors_screen_current_credentials() {
    let zero = include_str!("../../../tests/fixtures/network/quote_create_nat.json")
        .replace("712400", "0");
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json"),
        ),
        response("200 OK", &zero),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("refusing"))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", "package-1", "vpc-1");
    let failure = Client::new(&config)
        .quote_create_nat_instance(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::Unpriced));
    assert!(failure.error().api_error().is_none());
    assert!(!failure.error().to_string().contains("refusing"));
}

#[tokio::test]
async fn endpoint_rejects_short_credential_in_every_returned_identifier() {
    for field in ["endpointServiceId", "projectUuid", "zoneUuid"] {
        let body = format!(
            r#"{{"data":[{{"uuid":"endpoint-1","{field}":"q7"}}],"page":1,"size":10,"totalPage":1,"total":1}}"#
        );
        let server = Server::start(vec![response("200 OK", &body)]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("q7"))
            .with_project_id("proj-1".into());
        let config =
            Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
        let (ctx, _) = RequestContext::new();
        let input = ListEndpointsInput {
            zone_id: "zone-1".into(),
            ..Default::default()
        };
        assert!(
            Client::new(&config)
                .list_endpoints(&ctx, input)
                .await
                .is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn nat_inventory_rejects_encoded_credential_in_nested_ids() {
    let fixture = include_str!("../../../tests/fixtures/network/list_nat_instances.json");
    let cases = [
        ("\"uuid\": \"vpc-example\"", "vpc uuid"),
        ("\"projectUuid\": \"project-1\"", "projectUuid"),
        ("\"zoneUuid\": \"zone-example\"", "zoneUuid"),
        ("\"uuid\": \"package-example\"", "package uuid"),
        ("\"id\": \"package-1\"", "package id"),
        (
            "\"packageId\": \"nat-package-example\"",
            "package packageId",
        ),
        ("\"uuid\": \"image-example\"", "image uuid"),
        ("\"id\": \"image-1\"", "image id"),
        ("\"projectId\": \"project-1\"", "vpc projectId"),
    ];
    for (needle, label) in cases {
        let key = needle.split(':').next().unwrap();
        let body = fixture.replacen(needle, &format!("{key}: \"netcTcx\""), 1);
        assert_ne!(body, fixture, "{label}");
        let server = Server::start(vec![response("200 OK", &body)]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("q7"))
            .with_project_id("proj-1".into());
        let config =
            Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
        let (ctx, _) = RequestContext::new();
        let input = ListNatInstancesInput {
            zone_id: "zone-1".into(),
            ..Default::default()
        };
        assert!(
            Client::new(&config)
                .list_nat_instances(&ctx, input)
                .await
                .is_err(),
            "{label}"
        );
    }
}

#[tokio::test]
async fn server_rejects_encoded_credential_in_nested_identifiers() {
    for (field, parent) in [
        ("networkUuid", "internalInterfaces"),
        ("subnetUuid", "externalInterfaces"),
        ("portUuid", "internalInterfaces"),
        ("serverUuid", "externalInterfaces"),
        ("floatingIpId", "internalInterfaces"),
    ] {
        let body = format!(r#"[{{"uuid":"server-1","{parent}":[{{"{field}":"netcTcx"}}]}}]"#);
        let server = Server::start(vec![response("200 OK", &body)]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("q7"))
            .with_project_id("proj-1".into());
        let config =
            Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
        let (ctx, _) = RequestContext::new();
        assert!(
            Client::new(&config)
                .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
                .await
                .is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn nat_inventory_rejects_null_flavor_zone_element() {
    let fixture = include_str!("../../../tests/fixtures/network/list_nat_instances.json");
    let body = fixture.replacen("\"zone-example\"\n          ]", "null\n          ]", 1);
    assert_ne!(body, fixture);
    let server = Server::start(vec![response("200 OK", &body)]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListNatInstancesInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    assert!(
        Client::new(&config)
            .list_nat_instances(&ctx, input)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn nat_quote_allows_catalog_zone_outside_vpc_home_zones() {
    let vpcs = include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json")
        .replace("\"uuid\":\"az-1\"", "\"uuid\":\"az-home\"");
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
        ),
        response("200 OK", &vpcs),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
        ),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", "package-1", "vpc-1");
    assert!(
        Client::new(&config)
            .quote_create_nat_instance(&ctx, input)
            .await
            .is_ok()
    );
    assert_eq!(server.heads().len(), 5);
}

#[tokio::test]
async fn nat_quote_rejects_malformed_vpc_zones_before_posting() {
    for zones in [
        r#""zones":null"#,
        r#""Zones":[{"uuid":"az-1"}]"#,
        r#""zones":"unexpected""#,
        r#""zones":[{"uuid":"az-1","UUID":"other"}]"#,
        r#""zones":[{"uuid":"az-1"},{"uuid":"az-1"}]"#,
        r#""zones":[{"uuid":"az_1"}]"#,
        r#""zones":[{"uuid":"az 1"}]"#,
    ] {
        let vpcs = include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json")
            .replace(r#""zones":[{"uuid":"az-1"}]"#, zones);
        let server = Server::start(vec![
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
            ),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
            ),
            response("200 OK", &vpcs),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
            ),
        ]);
        let config = Config::new(with_product_loopback(
            Product::Vnetwork,
            options().with_project_id("proj-1".into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        let input =
            QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", "package-1", "vpc-1");
        let failure = Client::new(&config)
            .quote_create_nat_instance(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse",
            "{zones}"
        );
        assert_eq!(server.heads().len(), 4, "{zones}");
    }
}

#[tokio::test]
async fn nat_inventory_rejects_credential_in_failed_envelope() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"success":false,"code":"SafeError","message":"no","unknown":"toy-secret"}"#,
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListNatInstancesInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    let failure = Client::new(&config)
        .list_nat_instances(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn nat_inventory_rejects_null_nested_package_default() {
    let body = include_str!("../../../tests/fixtures/network/list_nat_instances.json")
        .replace("\"default\": true", "\"default\": null");
    let server = Server::start(vec![response("200 OK", &body)]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = ListNatInstancesInput {
        zone_id: "zone-1".into(),
        ..Default::default()
    };
    let failure = Client::new(&config)
        .list_nat_instances(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn nat_quote_rejects_colliding_nested_zone_ids() {
    let vpcs = include_str!("../../../tests/fixtures/network/nat_quote_vpcs.json")
        .replace("\"uuid\":\"az-1\"", "\"uuid\":\"az-1\",\"UUID\":\"other\"");
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_packages.json"),
        ),
        response("200 OK", &vpcs),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
        ),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", "package-1", "vpc-1");
    let failure = Client::new(&config)
        .quote_create_nat_instance(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.heads().len(), 4);
}
