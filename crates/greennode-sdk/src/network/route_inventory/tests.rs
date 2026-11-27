use crate::network::{
    Client, GetRouteTableInput, ListAllVirtualIpAddressAddressPairsInput, ListInterconnectsInput,
    ListVpnConnectionsInput, ListWanIpsInput,
};
use crate::testing::{Product, Response, Server, with_product_loopback};
use crate::{Config, ConfigOptions, ErrorClass, RequestContext, Secret};

#[tokio::test]
async fn route_inventory_regressions() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"listData":[{"uuid":"dG95"}]}"#,
    )]);
    let config = config(Product::Vserver, &server, "toy", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_wan_ips(&ctx, ListWanIpsInput::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn route_inventory_ignores_a_lone_case_variant_key() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"listData":[{"Name":"case"}]}"#,
    )]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert_eq!(
        client
            .list_wan_ips(&ctx, ListWanIpsInput::default())
            .await
            .unwrap()
            .items[0]
            .name,
        ""
    );
}

#[tokio::test]
async fn route_table_refuses_credentials_in_unknown_fields() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[]},"extra":"toy-token"}"#,
    )]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn route_table_accepts_underscore_in_project_id() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[]}}"#,
    )]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj_1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn vpn_accepts_nonempty_uuid_without_path_charset() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"success":true,"data":[{"uuid":"vpn.1"}],"page":1,"size":10,"totalPage":1,"total":1}"#,
    )]);
    let config = config(Product::Vnetwork, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_vpn_connections(&ctx, ListVpnConnectionsInput::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn truncated_not_found_is_a_body_read_failure() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 20\r\n\r\n{}".to_vec(),
    )]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let error = client
        .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
        .await
        .unwrap_err();
    assert_ne!(error.error().class(), ErrorClass::NotFound);
}

#[tokio::test]
async fn oversized_not_found_keeps_status() {
    let body = vec![b'x'; (8 << 20) + 1];
    let server = Server::start(vec![Response::Http {
        status: "404 Not Found".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let error = client
        .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NotFound);
}

#[tokio::test]
async fn vpn_cancellation_before_headers_preserves_class() {
    let (sent, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let config = config(Product::Vnetwork, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, cancel) = RequestContext::new();
    let cancel_when_started = async {
        ready.await.unwrap();
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(
        client.list_vpn_connections(&ctx, ListVpnConnectionsInput::default()),
        cancel_when_started
    );
    assert_eq!(result.unwrap_err().error().class(), ErrorClass::Cancelled);
}

#[tokio::test]
async fn cancellation_screens_operation_matching_bearer() {
    let (sent, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let token = "network.ListVPNConnections";
    let config = config(Product::Vnetwork, &server, token, Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, cancel) = RequestContext::new();
    let cancel_when_started = async {
        ready.await.unwrap();
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(
        client.list_vpn_connections(&ctx, ListVpnConnectionsInput::default()),
        cancel_when_started
    );
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Cancelled);
    assert!(
        !failure
            .error()
            .api_error()
            .unwrap()
            .operation()
            .contains(token)
    );
    assert!(!format!("{} {:?}", failure.error(), failure.error()).contains(token));
}

#[tokio::test]
async fn numeric_bearer_allows_nonmatching_inventory_numbers() {
    let vpn_server = Server::start(vec![response(
        "200 OK",
        r#"{"success":true,"data":[{"uuid":"vpn-1"}],"page":1,"size":10,"totalPage":1,"total":1}"#,
    )]);
    let vpn_config = config(Product::Vnetwork, &vpn_server, "937", Some("proj-1"));
    let client = Client::new(&vpn_config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_vpn_connections(&ctx, ListVpnConnectionsInput::default())
            .await
            .is_ok()
    );

    let link_server = Server::start(vec![response(
        "200 OK",
        r#"{"listData":[{"uuid":"link-1","circuitId":12}],"page":1,"pageSize":10,"totalPage":1,"totalItem":1}"#,
    )]);
    let config = config(Product::Vserver, &link_server, "937", Some("proj-1"));
    let client = Client::new(&config);
    assert!(
        client
            .list_interconnects(&ctx, ListInterconnectsInput::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn route_table_redirect_preserves_status_and_withholds_message() {
    let server = Server::start(vec![Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Location".into(), "/other".into()),
            ("Content-Length".into(), "0".into()),
        ],
        body: Vec::new(),
    }]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Api);
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(302), "", "security group response withheld")
    );
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn truncated_redirect_drops_status_and_reports_the_body_failure() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 302 Found\r\nLocation: /other\r\nContent-Length: 20\r\n\r\n{}".to_vec(),
    )]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), None);
    assert_eq!(failure.error().class(), ErrorClass::Api);
    assert_eq!(
        (api.code(), api.message()),
        (
            "",
            "network.GetRouteTable: request failed: network request failed"
        )
    );
    assert_eq!(server.heads().len(), 1);
}

#[test]
fn interconnect_circuit_id_is_screened_by_its_rendered_number() {
    let mut evidence = crate::wire::product::evidence::Evidence::new();
    evidence.add_secret(&Secret::new("e+21"));
    let interconnect: crate::network::Interconnect =
        serde_json::from_str(r#"{"circuitId":1e21}"#).unwrap();
    assert!(!evidence.text_reflects("1e21"));
    assert!(super::ids_reflect(&interconnect, &evidence));
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn config(product: Product, server: &Server, static_token: &str, project: Option<&str>) -> Config {
    let mut options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(static_token));
    if let Some(project) = project {
        options = options.with_project_id(project.into());
    }
    Config::new(with_product_loopback(product, options, server)).unwrap()
}

#[tokio::test]
async fn ordinary_lists_preserve_go_zero_values_and_redact_typed_strings() {
    let mut body =
        br#"{"listData":[{"uuid":"wan_1","name":"toy%2Dtoken","id":null,"status":""#.to_vec();
    body.push(0xff);
    body.extend_from_slice(br#"\uD800"}],"page":-0,"pageSize":null}"#);
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_wan_ips(
            &ctx,
            ListWanIpsInput {
                name: String::new(),
                page: -1,
                size: -2,
            },
        )
        .await
        .unwrap();
    assert_eq!(output.items[0].uuid, "wan_1");
    assert_eq!(output.items[0].name, "<redacted>");
    assert_eq!(output.items[0].id, "");
    assert_eq!(output.items[0].status, "��");
    assert_eq!((output.page, output.page_size), (0, 0));
    assert!(server.heads()[0].starts_with(b"GET /v2/proj-1/wanIps?name=&page=1&size=10000 "));
}

#[tokio::test]
async fn interconnect_refuses_numeric_credential_in_dynamic_value() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"listData":[{"uuid":"link_1","circuitId":{"%34%32-key":"drop","nested":[4.2e1,12],"safe":false}}],"page":1,"pageSize":2,"totalPage":1,"totalItem":1}"#,
    )]);
    let config = config(Product::Vserver, &server, "42", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_interconnects(&ctx, ListInterconnectsInput::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn route_table_distinguishes_syntax_shape_and_identity_failures() {
    let server = Server::start(vec![
        response("200 OK", r#"{"data":"#),
        response(
            "200 OK",
            r#"{"data":{"uuid":"rt-1","UUID":"rt-1","status":"ACTIVE","routes":[]}}"#,
        ),
        response(
            "200 OK",
            r#"{"data":{"uuid":"rt-2","status":"ACTIVE","routes":[]}}"#,
        ),
        response("404 Not Found", r#"{"message":"discard"}"#),
    ]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    for (status, code, message) in [
        (None, "", "response failed to decode; body withheld"),
        (
            None,
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        ),
        (
            None,
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        ),
        (Some(404), "NotFound", "security group response withheld"),
    ] {
        let failure = client
            .get_route_table(&ctx, GetRouteTableInput::new("rt-1"))
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(
            (api.status(), api.code(), api.message()),
            (status, code, message)
        );
        assert_eq!(api.operation(), "network.GetRouteTable");
    }
    assert_eq!(server.heads().len(), 4);
}

#[tokio::test]
async fn reflected_filters_are_refused_before_project_discovery() {
    let server = Server::start(vec![]);
    let config = config(Product::Vserver, &server, "toy-token", None);
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let input = ListWanIpsInput {
        name: " toy%2dtoken ".into(),
        ..Default::default()
    };
    let failure = client.list_wan_ips(&ctx, input).await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn address_pair_fanout_returns_no_partial_on_child_failure() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"listData":[{"uuid":"vip-1"},{"uuid":"vip-2"}],"page":1,"pageSize":10000,"totalPage":2,"totalItem":3}"#,
        ),
        response("200 OK", r#"{"data":[{"uuid":"pair_1"}]}"#),
        response("404 Not Found", "{}"),
    ]);
    let config = config(Product::Vserver, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let (error, partial, _) = client
        .list_all_virtual_ip_address_address_pairs(
            &ctx,
            ListAllVirtualIpAddressAddressPairsInput::default(),
        )
        .await
        .unwrap_err()
        .into_parts();
    assert!(partial.is_none());
    assert_eq!(error.class(), ErrorClass::NotFound);
    assert_eq!(
        error.api_error().unwrap().operation(),
        "network.ListAddressPairsByVirtualIPAddress"
    );
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[2].starts_with(b"GET /v2/proj-1/virtualIpAddress/vip-2/addressPairs "));
}

#[tokio::test]
async fn vpn_preserves_nullable_gateways_and_withholds_refusals() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"vpn_1","vpnGatewayIp":null,"vpnName":"toy%2dtoken","vpnSites":[{"tunnels":[]}]}],"page":1,"size":10,"totalPage":1,"total":1}"#,
        ),
        response(
            "200 OK",
            r#"{"success":true,"data":[{"uuid":"vpn-1","vpnSites":[{"tunnels":[{"tunnelName":null}]}]}],"page":1,"size":10,"totalPage":1,"total":1}"#,
        ),
        response(
            "200 OK",
            r#"{"success":false,"errorMessage":"opaque detail"}"#,
        ),
        response(
            "403 Forbidden",
            r#"{"code":"discard","message":"opaque detail"}"#,
        ),
    ]);
    let config = config(Product::Vnetwork, &server, "toy-token", Some("proj-1"));
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let input = ListVpnConnectionsInput::default();
    let output = client
        .list_vpn_connections(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(output.items[0].vpn_gateway_ip, None);
    assert_eq!(output.items[0].vpn_name, "<redacted>");
    for (status, code, message) in [
        (
            200,
            "InvalidResponse",
            "invalid VPN response; body withheld",
        ),
        (200, "", "VPN request failed; server details withheld"),
        (
            403,
            "Forbidden",
            "VPN request failed; server details withheld",
        ),
    ] {
        let failure = client
            .list_vpn_connections(&ctx, input.clone())
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(
            (api.status(), api.code(), api.message()),
            (Some(status), code, message)
        );
    }
    let failure = client
        .list_vpn_connections(&ctx, ListVpnConnectionsInput { page: -1, ..input })
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/proj-1/vpns?params=%7B%22search%22%3A%5B%7B%22field%22%3A%22any%22%2C%22value%22%3A%22%22%7D%5D%2C%22sort%22%3A%7B%7D%2C%22page%22%3A1%2C%22size%22%3A10%7D "));
}
