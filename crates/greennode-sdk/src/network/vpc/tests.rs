use super::models::*;
use crate::network::Client;
use crate::testing::{Product, Response, Server, with_product_loopback};
use crate::{Config, ConfigOptions, ErrorClass, RequestContext, Secret};

pub(super) fn reply(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

pub(super) fn client(server: &Server) -> Client {
    client_with_token(server, "toy-credential")
}

fn client_with_token(server: &Server, token: &str) -> Client {
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new(token)),
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
            r#"{"data":[{"id":"rule-1","securityGroupId":"other","description":"safe"}]}"#,
        ),
        reply("200 OK", r#"{"data":[]}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
        .await
        .unwrap();
    assert_eq!(output.items.len(), 1);
    assert_eq!(output.items[0].id, "rule-1");
    assert_eq!(output.items[0].security_group_id, "sg-1");
    assert_eq!(output.items[0].description, "safe");
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
        ("sg-1", r#"{"data":[{"id":"rule_1"}]}"#, 2),
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
        assert_eq!(
            api.operation(),
            if id == "toy-credential" {
                "network.ListSecurityGroups"
            } else {
                "network.ListAllSecurityGroupRules"
            }
        );
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
            r#"{"listData":[{"uuid":"acl_1","id":"other"}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#,
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
async fn vpc_rescreens_dns_identifier_using_its_wire_name() {
    let server = Server::start(vec![reply(
        "200 OK",
        r#"{"id":"vpc-1","dnsId":"pre6A6B6Cpost"}"#,
    )]);
    let result = client_with_token(&server, "jkl")
        .get_vpc(&RequestContext::new().0, GetVpcInput::new("vpc-1"))
        .await;
    assert_eq!(server.requests(), 1);
    assert!(result.is_err());
}

#[tokio::test]
async fn encoded_hex_credential_in_rendered_integer_is_rejected() {
    let server = Server::start(vec![reply("200 OK", r#"{"id":"vpc-1","mtu":-1500}"#)]);
    let (ctx, _) = RequestContext::new();
    assert!(
        client_with_token(&server, "2d31353030")
            .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn lone_page_case_variant_is_ignored() {
    let server = Server::start(vec![reply("200 OK", r#"{"listData":[],"Page":1}"#)]);
    let (ctx, _) = RequestContext::new();
    assert!(
        client(&server)
            .list_vpcs(&ctx, ListVpcsInput::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn lone_case_variant_is_ignored() {
    let server = Server::start(vec![reply("200 OK", r#"{"ID":"vpc-1"}"#)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap();
    assert!(output.vpc.uuid.is_empty());
}

#[tokio::test]
async fn omitted_zone_ranges_render_as_empty_array_with_null_token() {
    let server = Server::start(vec![reply("200 OK", r#"{"id":"vpc-1"}"#)]);
    let (ctx, _) = RequestContext::new();
    let output = client_with_token(&server, "null")
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(output).unwrap()["VPC"]["Zone"]["IPRanges"],
        serde_json::json!([])
    );
}

struct RotatingProvider(std::sync::atomic::AtomicUsize);

impl crate::CredentialsProvider for RotatingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>,
    > {
        Box::pin(async move {
            let calls = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::Token::new(
                Secret::new(if calls < 2 {
                    "first-token"
                } else {
                    "later-token"
                }),
                None,
            ))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), crate::Error>> + Send + 'a>>
    {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn aggregate_rechecks_earlier_rows_against_later_tokens() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"listData":[{"id":"sg-1"},{"id":"sg-2"}],"page":1,"pageSize":2,"totalPage":1,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"data":[{"id":"rule-1","description":"later-token"}]}"#,
        ),
        reply("200 OK", r#"{"data":[]}"#),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_credentials_provider(std::sync::Arc::new(RotatingProvider(
            std::sync::atomic::AtomicUsize::new(0),
        )));
    let config = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let (ctx, _) = RequestContext::new();
    let error = Client::new(&config)
        .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert!(error.partial().is_none());
}

#[tokio::test]
async fn rule_aggregate_limits_retained_output() {
    let payload = "x".repeat(4_300_000);
    let first = format!(r#"{{"data":[{{"id":"rule-1","description":"{payload}"}}]}}"#);
    let second = format!(r#"{{"data":[{{"id":"rule-2","description":"{payload}"}}]}}"#);
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"listData":[{"id":"sg-1"},{"id":"sg-2"}],"page":1,"pageSize":2,"totalPage":1,"totalItem":2}"#,
        ),
        reply("200 OK", &first),
        reply("200 OK", &second),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.code(), api.message(), api.status()),
        (
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
            None
        )
    );
    assert!(!error.error().is_body_too_large());
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn subnet_inventory_cannot_exceed_requested_page_size() {
    let rows = std::iter::repeat_n(r#"{"id":"vpc-1"}"#, 10001)
        .collect::<Vec<_>>()
        .join(",");
    let server = Server::start(vec![reply(
        "200 OK",
        &format!(r#"{{"listData":[{rows}]}}"#),
    )]);
    let (ctx, _) = RequestContext::new();
    assert!(
        client(&server)
            .list_subnets(&ctx, ListSubnetsInput::default())
            .await
            .is_err()
    );
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn returned_short_nonnumeric_identifier_is_rejected() {
    let server = Server::start(vec![reply("200 OK", r#"{"id":"xy"}"#)]);
    let (ctx, _) = RequestContext::new();
    assert!(
        client_with_token(&server, "xy")
            .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn returned_ids_and_rendered_numbers_screen_short_credentials() {
    let server = Server::start(vec![
        reply("200 OK", r#"{"id":"42"}"#),
        reply("200 OK", r#"{"id":"safe","mtu":4211}"#),
        reply(
            "200 OK",
            r#"{"listData":[{"id":"42"}],"page":1,"pageSize":3,"totalPage":1,"totalItem":1}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client_with_token(&server, "42");
    assert!(
        client
            .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
            .await
            .is_err()
    );
    assert!(
        client
            .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
            .await
            .is_err()
    );
    assert!(
        client
            .list_vpcs(&ctx, ListVpcsInput::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn protected_read_rejects_credential_in_unknown_field() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"data":{"id":"sg-1","unknown":"toy-credential"}}"#,
        ),
        reply(
            "200 OK",
            &format!(
                r#"{{"data":{{"id":"sg-1","unknown":"toy{}u002dcredential"}}}}"#,
                char::from(92)
            ),
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    for _ in 0..2 {
        let error = client
            .get_security_group(&ctx, GetSecurityGroupInput::new("sg-1"))
            .await
            .unwrap_err();
        assert!(error.partial().is_none());
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    }
}

#[tokio::test]
async fn protected_read_rejects_unicode_escaped_credential_character() {
    let body = format!(
        r#"{{"data":{{"id":"sg-1","unknown":"toy-{}u0041credential"}}}}"#,
        char::from(92)
    );
    let server = Server::start(vec![reply("200 OK", &body)]);
    let (ctx, _) = RequestContext::new();
    let error = client_with_token(&server, "toy-Acredential")
        .get_security_group(&ctx, GetSecurityGroupInput::new("sg-1"))
        .await
        .unwrap_err();
    assert!(error.partial().is_none());
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
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
        "invalid or incomplete inventory; body withheld",
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

fn assert_withheld(error: &crate::Failure<impl Sized>, code: &str, message: &str) {
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.code(), api.message(), api.status()),
        (code, message, None)
    );
}

#[tokio::test]
async fn inventory_page_echoing_a_credential_reports_the_protected_message() {
    let body = r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0,"extra":"toy-credential"}"#;
    let server = Server::start(vec![reply("200 OK", body), reply("200 OK", body)]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let error = client
        .list_security_groups(&ctx, ListSecurityGroupsInput::default())
        .await
        .unwrap_err();
    assert_withheld(
        &error,
        "InvalidResponse",
        "invalid floating IP response; body withheld",
    );
    let error = client
        .list_network_acls(&ctx, ListNetworkAclsInput::default())
        .await
        .unwrap_err();
    assert_withheld(
        &error,
        "InvalidResponse",
        "invalid floating IP response; body withheld",
    );
}

#[tokio::test]
async fn acl_detail_with_non_object_top_level_reports_inventory_error() {
    let server = Server::start(vec![reply("200 OK", "[]"), reply("200 OK", "\"text\"")]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        let error = client
            .get_network_acl(&ctx, GetNetworkAclInput::new("acl-1"))
            .await
            .unwrap_err();
        assert_withheld(
            &error,
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
        );
    }
}

#[test]
fn parse_acl_refuses_an_underscore_subnet_id() {
    let body = |subnet: &str| {
        format!(
            r#"{{"data":{{"uuid":"acl-1","status":"ACTIVE","defaultAcl":false,"interfaceNetworkUuid":"vpc-1","subnetAssociationList":["{subnet}"],"aclPolicyRules":[]}}}}"#
        )
    };
    assert!(
        super::policy::parse_acl(&body("sub-1"), "acl-1")
            .unwrap()
            .is_some()
    );
    assert!(
        super::policy::parse_acl(&body("sub_1"), "acl-1")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn long_token_containing_hex_of_zero_does_not_break_zero_numbers() {
    let server = Server::start(vec![reply(
        "200 OK",
        r#"{"id":"vpc-1","mtu":0,"serverCount":0,"zone":{"serverCount":0}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client_with_token(&server, "token-30-value")
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap();
    assert_eq!(output.vpc.mtu, 0);
}

#[tokio::test]
async fn subnet_and_vpc_zone_ignore_lone_case_variants() {
    let server = Server::start(vec![
        reply("200 OK", r#"{"UUID":"x","zone":{"UUID":"z"}}"#),
        reply("200 OK", r#"{"id":"vpc-1","zone":{"UUID":"z"}}"#),
        reply(
            "200 OK",
            r#"{"uuid":"sub-1","zone":{"uuid":"zone-1","Uuid":"a","UUID":"b"}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let subnet = client
        .get_subnet(&ctx, GetSubnetInput::new("vpc-1", "sub-1"))
        .await
        .unwrap();
    assert!(subnet.subnet.uuid.is_empty());
    assert!(subnet.subnet.zone_id.is_empty());
    let vpc = client
        .get_vpc(&ctx, GetVpcInput::new("vpc-1"))
        .await
        .unwrap();
    assert!(vpc.vpc.zone.uuid.is_empty());
    assert!(
        client
            .get_subnet(&ctx, GetSubnetInput::new("vpc-1", "sub-1"))
            .await
            .is_err()
    );
}
