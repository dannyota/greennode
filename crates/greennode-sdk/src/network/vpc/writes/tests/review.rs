use super::*;

const SUBNET: &str = r#"{"uuid":"subnet-1","networkUuid":"vpc-1","status":"ACTIVE"}"#;
const EMPTY_INVENTORY: &str =
    r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":0}"#;
const FIRST_INTERFACE_PAGE: &str = r#"{"listData":[{"uuid":"interface-1","subnetId":"subnet-2"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#;

#[tokio::test]
async fn subnet_delete_checks_every_interface_and_vip_page() {
    let server = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", "[]"),
        reply("200 OK", FIRST_INTERFACE_PAGE),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"interface-2","subnetUuid":"subnet-3"}],"page":2,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"vip-1","subnetId":"subnet-2"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"vip-2","subnetId":"subnet-3"}],"page":2,"pageSize":1,"totalPage":2,"totalItem":2}"#,
        ),
        reply("200 OK", EMPTY_INVENTORY),
        reply("200 OK", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteSubnetInput::new("vpc-1", "subnet-1");
    input.no_wait = true;
    client(&server).delete_subnet(&ctx, input).await.unwrap();
    assert_eq!(server.requests(), 8);
    assert!(server.heads().iter().any(|head| {
        String::from_utf8_lossy(head).contains("network-interfaces-elastic?name=&page=2&size=")
    }));
    assert!(
        server
            .heads()
            .iter()
            .any(|head| String::from_utf8_lossy(head)
                .contains("virtualIpAddress?name=&page=2&size="))
    );
}

#[tokio::test]
async fn subnet_delete_refuses_null_or_inconsistent_inventory_metadata() {
    let pages = [
        (
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":0,"totalItem":null}"#,
            None,
        ),
        (
            r#"{"listData":[],"page":1,"pageSize":null,"totalPage":0,"totalItem":0}"#,
            None,
        ),
        (
            r#"{"listData":[],"page":1,"pageSize":0,"totalPage":0,"totalItem":0}"#,
            None,
        ),
        (
            r#"{"listData":[],"page":1,"pageSize":1,"totalPage":null,"totalItem":0}"#,
            None,
        ),
        (
            FIRST_INTERFACE_PAGE,
            Some(
                r#"{"listData":[{"uuid":"interface-2"}],"page":2,"pageSize":2,"totalPage":2,"totalItem":2}"#,
            ),
        ),
        (
            FIRST_INTERFACE_PAGE,
            Some(r#"{"listData":[],"page":2,"pageSize":1,"totalPage":2,"totalItem":2}"#),
        ),
        (
            FIRST_INTERFACE_PAGE,
            Some(
                r#"{"listData":[{"uuid":"interface-2"}],"page":2,"pageSize":1,"totalPage":2,"totalItem":3}"#,
            ),
        ),
    ];
    for (first, second) in pages {
        let mut replies = vec![
            reply("200 OK", SUBNET),
            reply("200 OK", "[]"),
            reply("200 OK", first),
        ];
        if let Some(second) = second {
            replies.push(reply("200 OK", second));
        }
        let count = replies.len();
        replies.extend([
            reply("200 OK", EMPTY_INVENTORY),
            reply("200 OK", EMPTY_INVENTORY),
            reply("200 OK", ""),
        ]);
        let server = Server::start(replies);
        let (ctx, _) = RequestContext::new();
        let mut input = DeleteSubnetInput::new("vpc-1", "subnet-1");
        input.no_wait = true;
        let error = client(&server)
            .delete_subnet(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
        assert_eq!(server.requests(), count);
    }
}

#[tokio::test]
async fn vpc_delete_rejects_a_nested_zone_alias_before_any_write() {
    let server = Server::start(vec![
        reply("200 OK", r#"{"id":"vpc-1","zone":{"IsEnabled":true}}"#),
        reply("200 OK", "[]"),
        reply("200 OK", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteVpcInput::new("vpc-1");
    input.no_wait = true;
    let error = client(&server).delete_vpc(&ctx, input).await.unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn rename_confirmation_refuses_reflecting_display_fields() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        reply("200 OK", ""),
        reply(
            "200 OK",
            r#"{"id":"vpc-1","displayName":"renamed","zone":{"description":"toy%2dcredential"}}"#,
        ),
    ]);
    let failure = client(&server)
        .update_vpc(&ctx, UpdateVpcInput::new("vpc-1", "renamed"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    let partial = failure.partial().unwrap();
    assert_eq!(partial.vpc.name, "renamed");
    assert!(partial.vpc.zone.description.is_empty());
    assert_eq!(server.requests(), 2);

    let server = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", ""),
        reply(
            "200 OK",
            r#"{"uuid":"subnet-1","name":"renamed","interfaceAclPolicyName":"toy%2dcredential"}"#,
        ),
    ]);
    let failure = client(&server)
        .update_subnet(&ctx, UpdateSubnetInput::new("vpc-1", "subnet-1", "renamed"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    let partial = failure.partial().unwrap();
    assert_eq!(partial.subnet.name, "renamed");
    assert!(partial.subnet.interface_acl_policy_name.is_empty());
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn ambiguous_route_write_keeps_the_failed_confirmation_cause() {
    let initial = r#"{"data":{"uuid":"rt-1","status":"ACTIVE","routes":[]}}"#;
    let error_table = r#"{"data":{"uuid":"rt-1","status":"ERROR","routes":[]}}"#;
    let server = Server::start(vec![
        reply("200 OK", initial),
        reply("200 OK", initial),
        reply("503 Service Unavailable", ""),
        reply("200 OK", error_table),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .add_route(&ctx, AddRouteInput::new("rt-1", "10.0.1.0/24", "10.0.0.1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().class(), ErrorClass::NotSettled);
    assert_eq!(error.error().api_error().unwrap().status(), Some(503));
    assert!(error.error().has_identity(ErrorClass::Failed));
    let message = error
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(message.contains("route response withheld"));
    assert!(message.contains("route table rt-1 is ERROR"));
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn prewrite_wait_preserves_a_shorter_caller_deadline() {
    let server = Server::start(vec![]);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        with_clock(options(), clock.clone()),
        &server,
    ))
    .unwrap();
    let client = crate::network::Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let timeout = Duration::from_secs(1);
    let ctx = ctx.child_with_deadline(clock.monotonic_now() + timeout);
    let mut write = Write::new::<()>(
        &client,
        &ctx,
        crate::wire::product::evidence::Evidence::for_write(),
    )
    .unwrap();
    let error = write
        .active(
            "network.AddRoute",
            "route table rt-1",
            async |_, _| {
                clock.advance(timeout);
                Ok("PENDING")
            },
            |status| status,
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::DeadlineExceeded);
    assert_eq!(error.detail::<NetworkWriteError>(), None);
}

#[tokio::test]
async fn subnet_delete_does_not_reconcile_a_withheld_conflict() {
    let server = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", "[]"),
        reply("200 OK", EMPTY_INVENTORY),
        reply("200 OK", EMPTY_INVENTORY),
        reply("200 OK", EMPTY_INVENTORY),
        reply("409 Conflict", "{}"),
        reply("200 OK", EMPTY_INVENTORY),
    ]);
    let options = options().with_static_token(Secret::new("409"));
    let cfg = Config::new(with_product_loopback(Product::Vserver, options, &server)).unwrap();
    let mut input = DeleteSubnetInput::new("vpc-1", "subnet-1");
    input.no_wait = true;
    let failure = crate::network::Client::new(&cfg)
        .delete_subnet(&RequestContext::new().0, input)
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert!(api.status().is_none());
    assert!(api.observed_response_status() == Some(StatusCode::CONFLICT.as_u16()));
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn subnet_delete_requires_distinct_interface_and_vip_identities_across_pages() {
    for first_inventory in [true, false] {
        let mut replies = vec![reply("200 OK", SUBNET), reply("200 OK", "[]")];
        if !first_inventory {
            replies.push(reply("200 OK", EMPTY_INVENTORY));
        }
        let second = FIRST_INTERFACE_PAGE.replace("\"page\":1", "\"page\":2");
        replies.extend([
            reply("200 OK", FIRST_INTERFACE_PAGE),
            reply("200 OK", &second),
        ]);
        let expected = replies.len();
        replies.extend([
            reply("200 OK", EMPTY_INVENTORY),
            reply("200 OK", EMPTY_INVENTORY),
            reply("200 OK", ""),
        ]);
        let server = Server::start(replies);
        let mut input = DeleteSubnetInput::new("vpc-1", "subnet-1");
        input.no_wait = true;
        let failure = client(&server)
            .delete_subnet(&RequestContext::new().0, input)
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), expected);
    }
}

#[tokio::test]
async fn subnet_delete_accepts_clean_absence_of_an_acl_listed_earlier() {
    let server = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", "[]"),
        reply("200 OK", EMPTY_INVENTORY),
        reply("200 OK", EMPTY_INVENTORY),
        reply(
            "200 OK",
            r#"{"listData":[{"uuid":"acl-1","networkId":"vpc-1"}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#,
        ),
        reply("404 Not Found", "{}"),
        reply("200 OK", ""),
    ]);
    let mut input = DeleteSubnetInput::new("vpc-1", "subnet-1");
    input.no_wait = true;
    client(&server)
        .delete_subnet(&RequestContext::new().0, input)
        .await
        .unwrap();
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn dhcp_create_accepts_an_mtu_distinct_from_a_numeric_credential() {
    let server = Server::start(vec![reply(
        "201 Created",
        r#"{"data":{"uuid":"dhcp-1","name":"custom","mtu":1450}}"#,
    )]);
    let cfg = Config::new(with_product_loopback(
        Product::Vserver,
        options().with_static_token(Secret::new("409")),
        &server,
    ))
    .unwrap();
    let mut input = CreateDhcpOptionsInput::new("custom", vec!["192.0.2.1".into()]);
    input.mtu = Some(1450);
    let out = crate::network::Client::new(&cfg)
        .create_dhcp_options(&RequestContext::new().0, input)
        .await
        .unwrap();
    assert_eq!(out.dhcp_options.mtu, 1450);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn rename_confirmation_requires_canonical_fields() {
    let vpc = Server::start(vec![
        reply("200 OK", ""),
        reply("200 OK", r#"{"id":"vpc-1","Zone":{"description":"test"}}"#),
    ]);
    let subnet = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", ""),
        reply("200 OK", r#"{"uuid":"subnet-1","Name":"renamed"}"#),
    ]);
    let ctx = RequestContext::new().0;
    let vpc = client(&vpc)
        .update_vpc(&ctx, UpdateVpcInput::new("vpc-1", "renamed"))
        .await
        .unwrap_err();
    let subnet = client(&subnet)
        .update_subnet(&ctx, UpdateSubnetInput::new("vpc-1", "subnet-1", "renamed"))
        .await
        .unwrap_err();
    for error in [vpc.error(), subnet.error()] {
        assert!(error.has_class(ErrorClass::NotSettled));
    }
}
