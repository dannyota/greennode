use super::*;

async fn bandwidth_with_total_items_variant() -> bool {
    let server = Server::start(vec![response("200 OK", r#"{"data":[],"TotalItems":0}"#)]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    Client::new(&config)
        .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
        .await
        .is_err()
}

#[tokio::test]
async fn bandwidth_refuses_a_total_items_case_variant() {
    assert!(bandwidth_with_total_items_variant().await);
}

#[tokio::test]
async fn project_discovery_refuses_a_region_case_variant() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"projects":[{"projectId":"proj-1","Region":"han-1"}]}"#,
        ),
        response("200 OK", r#"{"data":[]}"#),
    ]);
    let config = Config::new(with_product_loopback(Product::Vserver, options(), &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(
        Client::new(&config)
            .list_bandwidth_packages(&ctx, ListBandwidthPackagesInput::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn nat_rules_refuse_paging_case_variants() {
    for body in [
        r#"{"success":true,"data":[],"Page":1}"#,
        r#"{"success":true,"data":[],"Total":5}"#,
        r#"{"success":true,"data":[],"NextPage":"more"}"#,
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let config = Config::new(with_product_loopback(
            Product::Vnetwork,
            options().with_project_id("proj-1".into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        let mut input = ListNatInboundRulesInput::new("nat-1");
        input.zone_id = "zone-1".into();
        assert!(
            Client::new(&config)
                .list_nat_inbound_rules(&ctx, input)
                .await
                .is_err(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn nat_region_discovery_refuses_case_variants() {
    for body in [
        r#"{"success":true,"data":[{"uuid":"zone-1","VnetworkDashboard":"https://hcm-3-vnetwork.console.greennode.ai","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
        r#"{"success":true,"data":[{"uuid":"zone-1","vnetworkDashboard":"https://hcm-3-vnetwork.console.greennode.ai","GatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
    ] {
        let server = Server::start(vec![
            response("200 OK", body),
            response(
                "200 OK",
                include_str!("../../../tests/fixtures/network/get_nat_instance.json"),
            ),
        ]);
        let config = Config::new(with_product_loopback(
            Product::Vnetwork,
            options().with_project_id("proj-1".into()),
            &server,
        ))
        .unwrap();
        let (ctx, _) = RequestContext::new();
        assert!(
            Client::new(&config)
                .get_nat_instance(&ctx, GetNatInstanceInput::new("nat-1"))
                .await
                .is_err(),
            "{body}"
        );
    }
}
