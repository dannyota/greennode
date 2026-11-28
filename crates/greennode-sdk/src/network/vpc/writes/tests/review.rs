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
async fn rename_confirmation_redacts_display_fields_without_losing_the_result() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        reply("200 OK", ""),
        reply(
            "200 OK",
            r#"{"id":"vpc-1","displayName":"renamed","zone":{"description":"toy%2dcredential"}}"#,
        ),
    ]);
    let output = client(&server)
        .update_vpc(&ctx, UpdateVpcInput::new("vpc-1", "renamed"))
        .await
        .unwrap();
    assert_eq!(output.vpc.name, "renamed");
    assert!(output.vpc.zone.description == "<redacted>");
    assert_eq!(server.requests(), 2);

    let server = Server::start(vec![
        reply("200 OK", SUBNET),
        reply("200 OK", ""),
        reply(
            "200 OK",
            r#"{"uuid":"subnet-1","name":"renamed","interfaceAclPolicyName":"toy%2dcredential"}"#,
        ),
    ]);
    let output = client(&server)
        .update_subnet(&ctx, UpdateSubnetInput::new("vpc-1", "subnet-1", "renamed"))
        .await
        .unwrap();
    assert_eq!(output.subnet.name, "renamed");
    assert!(output.subnet.interface_acl_policy_name == "<redacted>");
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
    let mut write = Write::new::<()>(&client, &ctx).unwrap();
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
