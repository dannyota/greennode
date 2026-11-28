use super::*;

pub(super) fn nat_detail_config(server: &Server, project: bool) -> Config {
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("xyz"));
    let options = if project {
        options.with_project_id("proj-1".into())
    } else {
        options
    };
    Config::new(with_product_loopback(Product::Vnetwork, options, server)).unwrap()
}

#[tokio::test]
async fn nat_detail_rejects_encoded_credential_in_vpc_and_package_ids() {
    let fixture = include_str!("../../../tests/fixtures/network/get_nat_instance.json");
    for needle in ["\"uuid\":\"vpc-1\"", "\"uuid\":\"pkg-1\""] {
        let body = fixture.replacen(needle, "\"uuid\":\"abceHl6def\"", 1);
        assert_ne!(body, fixture);
        let server = Server::start(vec![response("200 OK", &body)]);
        let config = nat_detail_config(&server, true);
        let (ctx, _) = RequestContext::new();
        let mut input = GetNatInstanceInput::new("nat-1");
        input.zone_id = "zone-1".into();
        assert_invalid(Client::new(&config).get_nat_instance(&ctx, input).await);
        assert_eq!(server.heads().len(), 1, "{needle}");
    }
}

#[tokio::test]
async fn nat_detail_rejects_underscore_in_vpc_id() {
    let body = include_str!("../../../tests/fixtures/network/get_nat_instance.json")
        .replace("\"uuid\":\"vpc-1\"", "\"uuid\":\"vpc_1\"");
    let server = Server::start(vec![response("200 OK", &body)]);
    let config = nat_detail_config(&server, true);
    let (ctx, _) = RequestContext::new();
    let mut input = GetNatInstanceInput::new("nat-1");
    input.zone_id = "zone-1".into();
    let failure = Client::new(&config)
        .get_nat_instance(&ctx, input)
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.code(), api.message()),
        ("InvalidResponse", "NAT response withheld")
    );
}

#[tokio::test]
async fn nat_detail_rejects_encoded_credential_in_discovered_project() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"abceHl6def","region":"hcm-3"}]}"#,
    )]);
    let nat = Server::start(vec![]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("xyz"));
    let options = with_product_loopback(Product::Vserver, options, &project);
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &nat)).unwrap();
    let (ctx, _) = RequestContext::new();
    let mut input = GetNatInstanceInput::new("nat-1");
    input.zone_id = "zone-1".into();
    assert_invalid(Client::new(&config).get_nat_instance(&ctx, input).await);
    assert_eq!(project.heads().len(), 1);
    assert!(nat.heads().is_empty());
}

#[tokio::test]
async fn nat_detail_rejects_encoded_credential_in_discovered_zone() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"success":true,"data":[{"uuid":"abceHl6def","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
    )]);
    let config = nat_detail_config(&server, true);
    let (ctx, _) = RequestContext::new();
    assert_invalid(
        Client::new(&config)
            .get_nat_instance(&ctx, GetNatInstanceInput::new("nat-1"))
            .await,
    );
    assert_eq!(server.heads().len(), 1);
}

async fn nat_list_failure(bodies: Vec<Response>) -> (Option<u16>, String, String, usize) {
    let server = Server::start(bodies);
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
    let api = failure.error().api_error().unwrap();
    (
        api.status(),
        api.code().to_string(),
        api.message().to_string(),
        server.heads().len(),
    )
}

#[tokio::test]
async fn nat_list_oversized_error_body_reports_status_text() {
    let big = "x".repeat(5 << 20);
    for (line, status, code, text) in [
        ("400 Bad Request", 400, "BadRequest", "Bad Request"),
        ("404 Not Found", 404, "NotFound", "Not Found"),
        (
            "503 Service Unavailable",
            503,
            "ServerError",
            "Service Unavailable",
        ),
    ] {
        assert_eq!(
            nat_list_failure(vec![response(line, &big)]).await,
            (Some(status), code.into(), text.into(), 1),
            "{line}"
        );
    }
}

#[tokio::test]
async fn nat_list_truncated_ok_body_is_a_statusless_read_failure() {
    let truncated =
        Response::RawClose(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"succ".to_vec());
    assert_eq!(
        nat_list_failure(vec![truncated]).await,
        (None, "".into(), "".into(), 1)
    );
}

#[tokio::test]
async fn nat_quote_placement_diagnostics_screen_current_credentials() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/nat_quote_whitelist.json"),
        ),
        response(
            "200 OK",
            include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
        ),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("requires"))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-x", "package-1", "vpc-1");
    let failure = Client::new(&config)
        .quote_create_nat_instance(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert!(failure.error().api_error().is_none());
    assert!(!failure.error().to_string().contains("requires"));
}

#[tokio::test]
async fn endpoint_refuses_credential_in_dynamic_package_and_portal_user_ids() {
    for (token, field) in [
        ("xyz", r#""packageId":{"uuid":"abceHl6def"}"#),
        ("xyz", r#""portalUserId":"abceHl6def""#),
        ("abc", r#""portalUserId":616263"#),
        ("abc", r#""packageId":["616263"]"#),
    ] {
        let body = format!(r#"{{"data":{{"uuid":"endpoint-1",{field}}}}}"#);
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
            ),
            response("200 OK", &body),
        ]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(token))
            .with_project_id("proj-1".into());
        let config =
            Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
        let (ctx, _) = RequestContext::new();
        let result = Client::new(&config)
            .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
            .await;
        let Err(failure) = result else {
            panic!("{field} accepted")
        };
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse",
            "{field}"
        );
    }
}

fn lone_variant_is_ignored<T: serde::de::DeserializeOwned + PartialEq + Default>(
    key: &str,
    variant: &str,
    value: &str,
) {
    let lone: T = serde_json::from_str(&format!(r#"{{"{variant}":{value}}}"#)).unwrap();
    assert!(lone == T::default(), "{key}");
    assert!(
        serde_json::from_str::<T>(&format!(r#"{{"{key}":{value},"{variant}":{value}}}"#)).is_err(),
        "{key}"
    );
}

#[test]
fn endpoint_models_ignore_lone_case_variants_and_reject_collisions() {
    lone_variant_is_ignored::<EndpointTag>("uuid", "UUID", r#""x""#);
    lone_variant_is_ignored::<VNetworkRegion>("uuid", "UUID", r#""x""#);
}

#[test]
fn nat_family_models_refuse_every_lone_case_variant() {
    assert!(serde_json::from_str::<NatInstance>(r#"{"UUID":"x"}"#).is_err());
    assert!(serde_json::from_str::<NatPackage>(r#"{"Default":true}"#).is_err());
    assert!(serde_json::from_str::<NatImage>(r#"{"ImageType":"x"}"#).is_err());
    assert!(serde_json::from_str::<NatPackageLimit>(r#"{"Cpu":2}"#).is_err());
    assert!(serde_json::from_str::<NatVpc>(r#"{"Name":"x"}"#).is_err());
    assert!(serde_json::from_str::<NatAvailabilityZone>(r#"{"Uuid":"x"}"#).is_err());
    assert!(serde_json::from_str::<NatOfferPrice>(r#"{"OptimumPrice":1}"#).is_err());
    assert!(serde_json::from_str::<NatPackageOffer>(r#"{"Uuid":"x"}"#).is_err());
    assert!(serde_json::from_str::<NetworkPriceProperty>(r#"{"Name":"x"}"#).is_err());
}

#[test]
fn output_accessor_ignores_lone_variants_while_guard_accessor_refuses() {
    let lone = super::wire::Fields::decode(r#"{"DATA":[]}"#).unwrap();
    assert!(lone.output_raw("data").unwrap().is_none());
    assert!(lone.raw("data").is_err());
    let both = super::wire::Fields::decode(r#"{"data":[],"DATA":[]}"#).unwrap();
    assert!(both.output_raw("data").is_err());
    assert!(both.raw("data").is_err());
}

#[tokio::test]
async fn bandwidth_guards_refuse_lone_case_variants() {
    let row = r#"{"uuid":"p1","name":"n","type":"t","description":"d","bandwidth":1}"#;
    for (extra, ok) in [
        (r#""total":1"#, true),
        (r#""TotalItems":1"#, false),
        (r#""HasMore":false"#, false),
        (r#""Success":true"#, false),
        (r#""Error":null"#, false),
    ] {
        let body = format!(r#"{{"data":[{row}],{extra}}}"#);
        let server = Server::start(vec![response("200 OK", &body)]);
        let config = Config::new(with_product_loopback(
            Product::Vserver,
            options().with_project_id("proj-1".into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        let result = Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await;
        if ok {
            assert!(result.is_ok(), "{extra}");
        } else {
            assert_invalid(result);
        }
    }
}

#[tokio::test]
async fn project_discovery_refuses_lone_region_variant() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"p1","Region":"han-1"}]}"#,
    )]);
    let config = Config::new(with_product_loopback(Product::Vserver, options(), &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
        .await;
    assert!(result.is_err());
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn nat_rule_ceilings_refuse_lone_case_variants() {
    for extra in [r#""Total":0"#, r#""TotalPage":1"#, r#""HasMore":false"#] {
        let body = format!(r#"{{"success":true,"data":[],{extra}}}"#);
        let server = Server::start(vec![response("200 OK", &body)]);
        let config = nat_detail_config(&server, true);
        let (ctx, _) = RequestContext::new();
        let mut input = ListNatInboundRulesInput::new("nat-1");
        input.zone_id = "zone-1".into();
        assert_invalid(
            Client::new(&config)
                .list_nat_inbound_rules(&ctx, input)
                .await,
        );
    }
}

#[tokio::test]
async fn nat_region_discovery_refuses_lone_case_variants() {
    for row in [
        r#"{"Uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}"#,
        r#"{"uuid":"zone-1","GatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}"#,
        r#"{"uuid":"zone-1","VnetworkDashboard":"https://hcm-3-vnetwork.console.greennode.ai"}"#,
    ] {
        let body = format!(r#"{{"success":true,"data":[{row}]}}"#);
        let server = Server::start(vec![response("200 OK", &body)]);
        let config = nat_detail_config(&server, true);
        let (ctx, _) = RequestContext::new();
        assert_invalid(
            Client::new(&config)
                .get_nat_instance(&ctx, GetNatInstanceInput::new("nat-1"))
                .await,
        );
    }
}

#[tokio::test]
async fn nat_detail_refuses_lone_case_variants_in_display_fields() {
    for (from, to) in [
        ("\"natName\"", "\"NatName\""),
        ("\"natGatewayIp\"", "\"NatGatewayIp\""),
        ("\"publicIp\"", "\"PublicIp\""),
        ("\"createdAt\"", "\"CreatedAt\""),
        ("\"name\":\"small\"", "\"Name\":\"small\""),
        ("\"name\":\"example-vpc\"", "\"Name\":\"example-vpc\""),
    ] {
        let body = include_str!("../../../tests/fixtures/network/get_nat_instance.json")
            .replace("\"ACTIVE\"", "\"PENDING\"")
            .replace(from, to);
        assert!(body.contains(to), "{from}");
        let server = Server::start(vec![response("200 OK", &body)]);
        let config = nat_detail_config(&server, true);
        let (ctx, _) = RequestContext::new();
        let mut input = GetNatInstanceInput::new("nat-1");
        input.zone_id = "zone-1".into();
        assert_invalid(Client::new(&config).get_nat_instance(&ctx, input).await);
    }
}

#[tokio::test]
async fn server_reads_ignore_lone_case_variants_and_reject_collisions() {
    let cases = [
        (
            r#"[{"uuid":"server-1","Name":"x","flavor":{"CPU":4}}]"#,
            true,
        ),
        (r#"[{"uuid":"server-1","name":"x","Name":"y"}]"#, false),
        (r#"[{"uuid":"server-1","flavor":{"cpu":1,"CPU":2}}]"#, false),
    ];
    for (body, ok) in cases {
        let server = Server::start(vec![response("200 OK", body)]);
        let config = Config::new(with_product_loopback(
            Product::Vserver,
            options().with_project_id("proj-1".into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        let result = Client::new(&config)
            .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
            .await;
        assert_eq!(result.is_ok(), ok, "{body}");
        if let Ok(list) = result {
            assert!(list.items[0].name.is_empty() && list.items[0].flavor.cpu == 0);
        }
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
async fn server_read(
    token: &str,
    body: &str,
    by_group: bool,
) -> Result<ListServersBySubnetOutput, crate::Failure<ListServersBySubnetOutput>> {
    let server = Server::start(vec![response("200 OK", body)]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(token))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let client = Client::new(&config);
    if by_group {
        client
            .list_servers_by_security_group(&ctx, ListServersBySecurityGroupInput::new("sg-1"))
            .await
    } else {
        client
            .list_servers_by_subnet(&ctx, ListServersBySubnetInput::new("subnet-1"))
            .await
    }
}

#[tokio::test]
async fn security_group_server_read_ignores_lone_case_variants() {
    let ok = server_read("zzq9k", r#"{"data":[{"uuid":"s1","Name":"x"}]}"#, true).await;
    assert!(ok.unwrap().items[0].name.is_empty());
    let both = server_read(
        "zzq9k",
        r#"{"data":[{"uuid":"s1","name":"x","Name":"y"}]}"#,
        true,
    )
    .await;
    let Err(failure) = both else {
        panic!("collision accepted")
    };
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.code(), api.message()),
        ("InvalidResponse", "invalid server response; body withheld")
    );
}

#[tokio::test]
async fn subnet_server_reflections_are_invalid_responses() {
    for (token, body) in [
        (
            "q7",
            r#"[{"uuid":"s1","internalInterfaces":[{"networkUuid":"netcTcx"}]}]"#,
        ),
        (
            "xyz",
            r#"[{"uuid":"s1","serverGroupId":{"id":"abceHl6def"}}]"#,
        ),
        ("abc", r#"[{"uuid":"s1","serverGroupId":616263}]"#),
        ("xyz", r#"[{"uuid":"s1","serverGroupId":["abceHl6def"]}]"#),
    ] {
        for by_group in [false, true] {
            let body = if by_group {
                format!(r#"{{"data":{body}}}"#)
            } else {
                body.to_string()
            };
            let Err(failure) = server_read(token, &body, by_group).await else {
                panic!("{body} accepted")
            };
            assert_eq!(
                failure.error().api_error().unwrap().code(),
                "InvalidResponse",
                "{body}"
            );
        }
    }
}

#[tokio::test]
async fn endpoint_refuses_credential_in_nested_ids_of_dynamic_objects() {
    for field in [
        "vpc",
        "subnet",
        "service",
        "apackage",
        "project",
        "region",
        "packages",
        "category",
        "secGroups",
        "subnets",
        "endpointResource",
        "elasticIps",
        "endpointDetailInformation",
        "endpointDomains",
    ] {
        let body = format!(
            r#"{{"data":{{"uuid":"endpoint-1","{field}":{{"inner":[{{"uuid":"abceHl6def"}}],"projectId":"abceHl6def"}}}}}}"#
        );
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
            ),
            response("200 OK", &body),
        ]);
        let config = nat_detail_config(&server, true);
        let (ctx, _) = RequestContext::new();
        let Err(failure) = Client::new(&config)
            .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
            .await
        else {
            panic!("{field} accepted")
        };
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse",
            "{field}"
        );
    }
}

#[tokio::test]
async fn quote_input_diagnostics_screen_current_credentials() {
    let server = Server::start(vec![]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("requires"))
        .with_project_id("proj-1".into());
    let config = Config::new(with_product_loopback(Product::Vnetwork, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let client = Client::new(&config);
    let nat = client
        .quote_create_nat_instance(
            &ctx,
            QuoteCreateNatInstanceInput::new("", "zone-1", "az-1", "package-1", "vpc-1"),
        )
        .await
        .unwrap_err();
    let endpoint = client
        .quote_create_endpoint(&ctx, QuoteCreateEndpointInput::new("", "vpc-1", "subnet-1"))
        .await
        .unwrap_err();
    assert!(nat.error().api_error().is_none());
    assert!(!nat.error().to_string().contains("requires"));
    assert!(
        !endpoint
            .error()
            .api_error()
            .unwrap()
            .message()
            .contains("requires")
    );
}

#[tokio::test]
async fn nat_catalog_lookups_keep_the_callers_withheld_error() {
    for project_configured in [false, true] {
        let server = Server::start(vec![Response::RawClose(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 8\r\n\r\n{}".to_vec(),
        )]);
        let mut options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new("xyz"));
        if project_configured {
            options = options.with_project_id("proj-1".into());
            options = with_product_loopback(Product::Vnetwork, options, &server);
        } else {
            options = with_product_loopback(Product::Vserver, options, &server);
        }
        let config = Config::new(options).unwrap();
        let (ctx, _) = RequestContext::new();
        let failure = Client::new(&config)
            .list_nat_zones(&ctx, ListNatZonesInput::default())
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(
            (api.operation(), api.status(), api.code(), api.message()),
            ("network.ListNATZones", None, "", "NAT response withheld")
        );
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn nat_catalog_project_inventory_screens_unknown_fields_before_catalog_reads() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3"}],"unknown":"toy-secret"}"#,
    )]);
    let nat = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        with_product_loopback(Product::Vserver, options(), &project),
        &nat,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: "zone-1".into(),
            },
        )
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code(), api.message()),
        (
            "network.ListNATZones",
            Some(200),
            "InvalidResponse",
            "NAT response withheld"
        )
    );
    assert_eq!(project.requests(), 1);
    assert_eq!(nat.requests(), 0);
}

#[tokio::test]
async fn nat_catalog_project_inventory_accepts_limit_metadata() {
    let project = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"proj-1","region":"hcm-3"}],"limit":100}"#,
    )]);
    let nat = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/list_nat_zones.json"),
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        with_product_loopback(Product::Vserver, options(), &project),
        &nat,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let outcome = Client::new(&config)
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: "zone-1".into(),
            },
        )
        .await;
    assert!(outcome.is_ok());
    assert_eq!(project.requests(), 1);
    assert_eq!(nat.requests(), 1);
}

#[tokio::test]
async fn oversized_nat_region_lookup_reports_the_regions_envelope() {
    let server = Server::start(vec![Response::RawClose(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            (4 << 20) + 1
        )
        .into_bytes(),
    )]);
    let config = nat_detail_config(&server, true);
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .list_nat_instances(&ctx, ListNatInstancesInput::default())
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code(), api.message()),
        (
            "network.ListNATInstances",
            Some(200),
            "InvalidResponse",
            "oversized regions envelope"
        )
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn security_group_read_ignores_a_lone_alias_inside_data() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":{"id":"group-1","Name":"example"}}"#,
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let result = Client::new(&config)
        .get_security_group(
            &RequestContext::new().0,
            GetSecurityGroupInput::new("group-1"),
        )
        .await
        .unwrap();
    assert_eq!(result.security_group.id, "group-1");
    assert!(result.security_group.name.is_empty());
}
