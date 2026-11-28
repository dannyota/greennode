use super::*;

#[tokio::test]
async fn direct_vpc_and_endpoint_404s_keep_the_bound_proof() {
    let ctx = RequestContext::new().0;
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"code":"VPCMissing","message":"provider \ud800 missing"}"#,
    )]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let failure = Client::new(&config)
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        failure.error(),
        "network.GetVPC",
        "GET",
        "v2/{}/networks/{}"
    ));
    assert_eq!(failure.error().api_error().unwrap().code(), "VPCMissing");
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"zone-1","gatewayUrl":"https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/"}]}"#,
        ),
        response("404 Not Found", "{}"),
    ]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let failure = Client::new(&config)
        .get_endpoint(&ctx, GetEndpointInput::new("endpoint-1"))
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        failure.error(),
        "network.GetEndpoint",
        "GET",
        "vnetwork/v1/{}/{}/endpoints/{}"
    ));
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn protected_network_reads_keep_the_classified_404_proof() {
    let server = Server::start((0..3).map(|_| response("404 Not Found", "{}")).collect());
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let client = Client::new(&config);
    let ctx = RequestContext::new().0;
    let group = client
        .get_security_group(&ctx, GetSecurityGroupInput::new("group-1"))
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        group.error(),
        "network.GetSecurityGroup",
        "GET",
        "v2/{}/secgroups/{}"
    ));
    let acl = client
        .get_network_acl(&ctx, GetNetworkAclInput::new("acl-1"))
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        acl.error(),
        "network.GetNetworkACL",
        "GET",
        "v2/{}/network-acl/{}"
    ));
    let route = client
        .get_route_table(&ctx, GetRouteTableInput::new("route-1"))
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        route.error(),
        "network.GetRouteTable",
        "GET",
        "v2/{}/route-table/{}"
    ));
    assert_eq!(server.requests(), 3);
    let server = Server::start(vec![response("404 Not Found", "{}")]);
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options().with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let vpn = Client::new(&config)
        .list_vpn_connections(&ctx, ListVpnConnectionsInput::default())
        .await
        .unwrap_err();
    assert!(crate::confirms_absence(
        vpn.error(),
        "network.ListVPNConnections",
        "GET",
        "vnetwork/v1/{}/vpns"
    ));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn protected_network_rebuild_preserves_class_when_the_status_is_hidden() {
    let server = Server::start(vec![response("404 Not Found", "{}")]);
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        options()
            .with_static_token(Secret::new("404"))
            .with_project_id("proj-1".into()),
        &server,
    ))
    .unwrap();
    let failure = Client::new(&config)
        .get_security_group(
            &RequestContext::new().0,
            GetSecurityGroupInput::new("group-1"),
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(failure.error().api_error().unwrap().status(), None);
    assert!(crate::confirms_absence(
        failure.error(),
        "network.GetSecurityGroup",
        "GET",
        "v2/{}/secgroups/{}"
    ));
}
