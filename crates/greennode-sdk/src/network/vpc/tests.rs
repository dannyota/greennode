use super::models::*;
use crate::network::Client;
use crate::testing::{Product, Response, Server, with_product_loopback};
use crate::{Config, ConfigOptions, ErrorClass, RequestContext, Secret};

fn reply(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server) -> Client {
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new("toy-credential")),
        server,
    ))
    .unwrap();
    Client::new(&config)
}

#[tokio::test]
async fn security_group_aggregate_reads_all_pages_before_children() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"listData":[{"id":"sg-1"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"id":"sg-2"}],"page":2,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"data":[{"id":"rule_1","securityGroupId":"other","description":"toy%2Dcredential"}]}"#,
        ),
        reply("200 OK", r#"{"data":[]}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
        .await
        .unwrap();
    assert_eq!(output.items.len(), 1);
    assert_eq!(output.items[0].id, "rule_1");
    assert_eq!(output.items[0].security_group_id, "sg-1");
    assert_eq!(output.items[0].description, "<redacted>");
    let expected = [
        "GET /v2/proj-1/secgroups?name=&page=1&size=10000 ",
        "GET /v2/proj-1/secgroups?name=&page=2&size=10000 ",
        "GET /v2/proj-1/secgroups/sg-1/secGroupRules ",
        "GET /v2/proj-1/secgroups/sg-2/secGroupRules ",
    ];
    let heads = server.heads();
    assert_eq!(heads.len(), expected.len());
    assert!(
        heads
            .iter()
            .zip(expected)
            .all(|(head, request)| head.starts_with(request.as_bytes()))
    );
}

#[tokio::test]
async fn aggregate_rejects_missing_child_inventory_and_path_credentials() {
    for (id, child, calls) in [
        ("sg-1", r#"{"data":null}"#, 2),
        ("sg-1", r#"{"data":[{"id":"rule-1"},{"id":"rule-1"}]}"#, 2),
        ("toy-credential", r#"{"data":[]}"#, 1),
    ] {
        let server = Server::start(vec![
            reply(
                "200 OK",
                &format!(
                    r#"{{"listData":[{{"id":"{id}"}}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}}"#
                ),
            ),
            reply("200 OK", child),
        ]);
        let (ctx, _) = RequestContext::new();
        let error = client(&server)
            .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
            .await
            .unwrap_err();
        assert!(error.partial().is_none());
        let api = error.error().api_error().unwrap();
        assert_eq!(api.operation(), "network.ListAllSecurityGroupRules");
        assert_eq!(api.code(), "InvalidResponse");
        assert_eq!(api.status(), None);
        assert_eq!(server.heads().len(), calls);
    }
}

#[tokio::test]
async fn acl_server_error_requires_complete_absence_before_not_found() {
    for (inventory, class) in [
        (
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
            ErrorClass::NotFound,
        ),
        (
            r#"{"listData":[{"uuid":"acl_1","id":"acl-1"}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#,
            ErrorClass::Api,
        ),
        (
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#,
            ErrorClass::Api,
        ),
    ] {
        let server = Server::start(vec![
            reply(
                "500 Internal Server Error",
                r#"{"code":"E500","message":"toy%2Dcredential"}"#,
            ),
            reply("200 OK", inventory),
        ]);
        let (ctx, _) = RequestContext::new();
        let error = client(&server)
            .get_network_acl(&ctx, GetNetworkAclInput::new("acl-1"))
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), class);
        if class == ErrorClass::Api {
            let api = error.error().api_error().unwrap();
            assert_eq!(
                (api.status(), api.code(), api.message()),
                (Some(500), "E500", "security group response withheld")
            );
        }
        let heads = server.heads();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].starts_with(b"GET /v2/proj-1/network-acl/acl-1 "));
        assert!(heads[1].starts_with(b"GET /v2/proj-1/network-acl/list?name=&page=1&size=10000 "));
    }
}

#[tokio::test]
async fn inventory_separates_syntax_from_shape_and_screens_query_inputs() {
    let server = Server::start(vec![
        reply("200 OK", "{"),
        reply(
            "200 OK",
            r#"{"listData":null,"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[],"page":1,"Page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"id":"vpc-1","mtu":-0,"zone":{"name":"toy%2Dcredential"}}],"page":2,"pageSize":5,"totalPage":2,"totalItem":1}"#,
        ),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    for (code, message) in [
        ("", "response failed to decode; body withheld"),
        (
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        ),
        (
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        ),
    ] {
        let error = client
            .list_security_groups(&ctx, ListSecurityGroupsInput::default())
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(
            (api.code(), api.message(), api.status()),
            (code, message, None)
        );
    }
    let input = ListVpcsInput {
        name: "toy%2Dcredential".into(),
        ..ListVpcsInput::default()
    };
    let error = client.list_vpcs(&ctx, input).await.unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    let input = ListVpcsInput {
        name: "two words &".into(),
        page: 2,
        size: 5,
    };
    let output = client.list_vpcs(&ctx, input).await.unwrap();
    assert_eq!(output.items[0].mtu, 0);
    assert_eq!(output.items[0].zone.name, "<redacted>");
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[3].starts_with(b"GET /v2/proj-1/networks?name=two+words+%26&page=2&size=5 "));
}

#[tokio::test]
async fn wire_errors_match_plain_and_protected_read_contracts() {
    let server = Server::start(vec![
        reply("200 OK", r#"{"mtu":{"$serde_json::private::Number":"12"}}"#),
        reply("403 Forbidden", r#"{"message":"permission denied"}"#),
        Response::Http {
            status: "307 Temporary Redirect".into(),
            headers: vec![
                ("Location".into(), "/v2/proj-1/secgroups/sg-2".into()),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let error = client
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (
            None,
            "",
            "network.GetVPC: request failed: response failed to decode"
        )
    );
    let error = client
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(403), "Forbidden", "permission denied")
    );
    let error = client
        .get_security_group(&ctx, GetSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(307), "", "security group response withheld")
    );
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn acl_detail_distinguishes_invalid_types_from_missing_guard_state() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"data":{"uuid":"acl-1","status":"ACTIVE","defaultAcl":"false","interfaceNetworkUuid":"vpc-1","subnetAssociationList":[],"aclPolicyRules":[]}}"#,
        ),
        reply(
            "200 OK",
            r#"{"data":{"uuid":"acl-1","status":"ACTIVE","interfaceNetworkUuid":"vpc-1","subnetAssociationList":[],"aclPolicyRules":[]}}"#,
        ),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    for message in [
        "invalid floating IP response; body withheld",
        "invalid or incomplete inventory; body withheld",
    ] {
        let error = client
            .get_network_acl(&ctx, GetNetworkAclInput::new("acl-1"))
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(
            (api.code(), api.message(), api.status()),
            ("InvalidResponse", message, None)
        );
    }
    assert_eq!(server.heads().len(), 2);
}
