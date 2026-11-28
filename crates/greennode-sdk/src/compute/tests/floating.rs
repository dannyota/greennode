use super::*;
use crate::{CredentialsProvider, Error, Token};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const GROUP: &str = r#"{"data":{"id":"sg-1","name":"firewall"}}"#;
const SAFE: &str = r#"{"data":[{"direction":"ingress","etherType":"IPv4","protocol":"TCP","portRangeMin":22,"portRangeMax":22,"remoteIpPrefix":"192.0.2.8/32","remoteGroupId":"sg-1"}]}"#;
const QUOTE: &str = include_str!("../../../tests/fixtures/compute/server-order-quote.json");
const LIST: &str = include_str!("../../../tests/fixtures/compute/server-order-inventory.json");
const ACK: &str = include_str!("../../../tests/fixtures/compute/server-order-accepted.json");

fn floating_input() -> CreateServerInput {
    CreateServerInput {
        name: "example".into(),
        zone_id: "zone-1".into(),
        flavor_id: "flavor-1".into(),
        image_id: "image-1".into(),
        vpc_id: "vpc-1".into(),
        subnet_id: "subnet-1".into(),
        security_group_ids: vec!["sg-1".into()],
        root_disk_size: 20,
        root_disk_type_id: "disk-1".into(),
        max_price: 20.0,
        ssh_key_id: "key-1".into(),
        attach_floating_ip: true,
        ..Default::default()
    }
}

fn floating_client(server: &TestServer, options: ConfigOptions) -> Client {
    let config = Config::new(
        options
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

fn key_options() -> ConfigOptions {
    ConfigOptions::default().with_static_token(Secret::new(CANARY))
}

const ROUTES: &[&str] = &[
    "GET /v2/project-1/servers?page=1&size=10000 ",
    "GET /v2/project-1/secgroups/sg-1 ",
    "GET /v2/project-1/secgroups/sg-1/secGroupRules ",
    "POST /portal/v1/price ",
    "GET /v2/project-1/secgroups/sg-1 ",
    "GET /v2/project-1/secgroups/sg-1/secGroupRules ",
    "POST /v2/project-1/servers ",
    "GET /v2/project-1/servers/server-1 ",
    "GET /v2/project-1/servers/server-1 ",
];

fn routes(server: &TestServer, count: usize) {
    let heads = server.heads();
    assert_eq!(heads.len(), count);
    assert!(
        heads
            .iter()
            .zip(ROUTES)
            .all(|(head, route)| head.starts_with(route.as_bytes()))
    );
}

#[tokio::test]
async fn floating_create_checks_both_snapshots_and_confirms_the_address() {
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(
            r#"{"data":{"uuid":"server-1","status":"ACTIVE","internalInterfaces":[{"floatingIp":"192.0.2.11","floatingIpId":"ip-1"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.security_group_ids.push("sg-1".into());
    let output = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.server.public_ip, "192.0.2.11");
    assert_eq!(output.monthly_price, 20.0);
    routes(&server, 8);
    let bodies = server.bodies();
    let order: serde_json::Value = serde_json::from_slice(&bodies[6]).unwrap();
    assert_eq!(order["attachFloating"], true);
    assert!(order.get("allowOpenLogin").is_none());
}

#[tokio::test]
async fn duplicate_guard_refuses_reflected_unknown_inventory_and_case_variants() {
    for inventory in [
        format!(
            r#"{{"listData":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":0,"extra":{{"nestedId":"{}"}}}}"#,
            CANARY
        ),
        r#"{"ListData":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":0}"#.to_owned(),
    ] {
        let server = TestServer::start(vec![response(&inventory)]);
        let (ctx, _) = RequestContext::new();
        let failure = floating_client(&server, key_options())
            .create_server(&ctx, floating_input())
            .await
            .unwrap_err();
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse"
        );
        assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    }
}

#[tokio::test]
async fn changed_floating_snapshot_refuses_before_order_even_with_override() {
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(r#"{"data":{"id":"sg-1","name":"firewall","newProperty":true}}"#),
        response(SAFE),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.allow_open_login = true;
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert!(failure.error().detail::<UnexpectedStatus>().is_some());
    routes(&server, 6);
}

#[tokio::test]
async fn override_refuses_null_rule_documents() {
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(r#"{"data":null}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.allow_open_login = true;
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    routes(&server, 3);
}

struct Rotating(AtomicUsize, usize);
impl CredentialsProvider for Rotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(Token::new(
                Secret::new(format!(
                    "toy-rotating-bearer-{}",
                    (self.0.fetch_add(1, Ordering::SeqCst) + 1).min(self.1)
                )),
                None,
            ))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn later_bearers_recheck_the_retained_quote_before_creating() {
    for reflected in 5..=7 {
        let quote = QUOTE.replace("INSTANCE", &format!("toy-rotating-bearer-{reflected}"));
        let mut replies = vec![
            response(LIST),
            response(GROUP),
            response(SAFE),
            response(&quote),
            response(GROUP),
            response(SAFE),
        ];
        replies.truncate(reflected - 1);
        let server = TestServer::start(replies);
        let (ctx, _) = RequestContext::new();
        let options = ConfigOptions::default()
            .with_credentials_provider(Arc::new(Rotating(AtomicUsize::new(0), usize::MAX)));
        let failure = floating_client(&server, options)
            .create_server(&ctx, floating_input())
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse"
        );
        assert!(failure.partial().is_none());
        routes(&server, reflected - 1);
    }
}

#[tokio::test]
async fn truncated_floating_order_keeps_ambiguous_create_advice() {
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        Response::RawClose(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 100\r\n\r\n{".to_vec()),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert!(
        failure
            .error()
            .api_error()
            .is_none_or(|api| api.status().is_none())
    );
    let advice = failure
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(advice.contains("may have already reached"));
    assert!(!advice.contains("disk type"));
}

#[tokio::test]
async fn floating_create_withholds_reflected_output_field_name() {
    struct Late(AtomicUsize);
    impl CredentialsProvider for Late {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            Box::pin(async move {
                let token = if self.0.fetch_add(1, Ordering::SeqCst) < 7 {
                    CANARY
                } else {
                    "MonthlyPrice"
                };
                Ok(Token::new(Secret::new(token), None))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let quote = r#"{"optimumPrice":20,"originalPrice":25,"discountPrice":5,"discountPercent":20,"propertiesPrice":[]}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(quote),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(
            r#"{"data":{"uuid":"server-1","status":"ACTIVE","internalInterfaces":[{"floatingIp":"192.0.2.11","floatingIpId":"ip-1"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let options =
        ConfigOptions::default().with_credentials_provider(Arc::new(Late(AtomicUsize::new(0))));
    let failure = floating_client(&server, options)
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
}

#[tokio::test]
async fn ordinary_create_screens_output_against_user_data() {
    let quote = r#"{"optimumPrice":12345678,"originalPrice":12345678,"discountPrice":0,"discountPercent":0,"propertiesPrice":[]}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(quote),
        http("202 Accepted", ACK),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.attach_floating_ip = false;
    input.no_wait = true;
    input.max_price = 12345679.0;
    input.ssh_key_id.clear();
    input.user_data = Some(Secret::new(" 12345678.00 "));
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
}

#[tokio::test]
async fn floating_address_failure_keeps_the_last_matching_server_and_price() {
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE","name":"observed"}}"#),
        response(
            r#"{"data":{"uuid":"server-2","status":"ACTIVE","internalInterfaces":[{"floatingIp":"192.0.2.11","floatingIpId":"ip-1"}]}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    let partial = failure.partial().unwrap();
    assert_eq!(partial.server.uuid, "server-1");
    assert_eq!(partial.server.name, "observed");
    assert!(partial.server.public_ip.is_empty());
    assert_eq!(partial.monthly_price, 20.0);
    routes(&server, 9);
}

#[tokio::test]
async fn floating_login_guard_combines_ipv4_and_ipv6_ingress_ranges_per_family() {
    for (family, first, second) in [
        ("IPv4", "0.0.0.0/1", "128.0.0.0/1"),
        ("IPv6", "::/1", "8000::/1"),
    ] {
        let rule = |prefix: &str| {
            serde_json::json!({"data":[{"direction":"ingress","etherType":family,"protocol":"TCP","portRangeMin":22,"portRangeMax":22,"remoteIpPrefix":prefix,"remoteGroupId":"sg-1"}]}).to_string()
        };
        let server = TestServer::start(vec![
            response(LIST),
            response(GROUP),
            response(&rule(first)),
            response(r#"{"data":{"id":"sg-2"}}"#),
            response(&rule(second)),
        ]);
        let (ctx, _) = RequestContext::new();
        let mut input = floating_input();
        input.security_group_ids.push("sg-2".into());
        let failure = floating_client(&server, key_options())
            .create_server(&ctx, input)
            .await
            .unwrap_err();
        assert!(failure.error().detail::<OpenLogin>().is_some());
        assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
        let heads = server.heads();
        assert_eq!(heads.len(), 5);
        assert!(heads[0].starts_with(ROUTES[0].as_bytes()));
        assert!(heads[1].starts_with(ROUTES[1].as_bytes()));
        assert!(heads[2].starts_with(ROUTES[2].as_bytes()));
        assert!(heads[3].starts_with(b"GET /v2/project-1/secgroups/sg-2 "));
        assert!(heads[4].starts_with(b"GET /v2/project-1/secgroups/sg-2/secGroupRules "));
    }
}

#[tokio::test]
async fn floating_create_accepts_256_distinct_bearers_and_refuses_the_257th() {
    for limit in [256, 257] {
        let mut input = floating_input();
        input.no_wait = true;
        input.security_group_ids = (0..64).map(|index| format!("sg-{index}")).collect();
        let mut replies = vec![response(LIST)];
        let mut expected = vec![ROUTES[0].to_owned()];
        for pass in 0..2 {
            if pass == 1 {
                replies.push(response(QUOTE));
                expected.push(ROUTES[3].into());
            }
            for id in &input.security_group_ids {
                replies.push(response(&serde_json::json!({"data":{"id":id}}).to_string()));
                replies.push(response(SAFE));
                expected.push(format!("GET /v2/project-1/secgroups/{id} "));
                expected.push(format!("GET /v2/project-1/secgroups/{id}/secGroupRules "));
            }
        }
        if limit == 256 {
            replies.push(http("202 Accepted", ACK));
            expected.push(ROUTES[6].into());
        } else {
            replies.truncate(256);
            expected.truncate(256);
        }
        let server = TestServer::start(replies);
        let provider = Arc::new(Rotating(AtomicUsize::new(0), limit));
        let options = ConfigOptions::default().with_credentials_provider(provider.clone());
        let (ctx, _) = RequestContext::new();
        let result = floating_client(&server, options)
            .create_server(&ctx, input)
            .await;
        if limit == 256 {
            assert_eq!(result.unwrap().server.uuid, "server-1");
        } else {
            let failure = result.unwrap_err();
            assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
            assert_eq!(
                failure.error().api_error().unwrap().code(),
                "InvalidResponse"
            );
            assert!(failure.partial().is_none());
        }
        let heads = server.heads();
        assert_eq!(heads.len(), expected.len());
        assert!(
            heads
                .iter()
                .zip(&expected)
                .all(|(head, route)| head.starts_with(route.as_bytes()))
        );
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            if limit == 256 { expected.len() } else { 257 }
        );
    }
}

#[tokio::test]
async fn floating_address_wait_bounds_an_in_flight_request_to_two_minutes() {
    let (started, read_started) = tokio::sync::oneshot::channel();
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(r#"{"data":{"uuid":"server-1","status":"ACTIVE"}}"#),
        Response::StallBeforeHead(started),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = floating_client(&server, key_options());
    let order = client.create_server(&ctx, floating_input());
    let advance = async {
        read_started.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(120)).await;
    };
    let (result, ()) = tokio::join!(order, advance);
    let failure = result.unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(failure.error().has_identity(ErrorClass::DeadlineExceeded));
    let partial = failure.partial().unwrap();
    assert_eq!(partial.server.uuid, "server-1");
    assert!(partial.server.public_ip.is_empty());
    assert_eq!(partial.monthly_price, 20.0);
    routes(&server, 9);
}

#[tokio::test]
async fn regression_null_port_is_not_a_valid_restricted_rule() {
    let rules = SAFE.replace(
        "\"portRangeMin\":22,\"portRangeMax\":22",
        "\"portRangeMin\":null,\"portRangeMax\":65535",
    );
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(&rules),
        response(QUOTE),
        response(GROUP),
        response(&rules),
        http("202 Accepted", ACK),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.no_wait = true;
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().detail::<OpenLogin>().is_some());
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn regression_missing_pagination_is_invalid_input() {
    let server = TestServer::start(vec![response(
        r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":1}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
}

#[tokio::test]
async fn floating_accepts_negative_zero_port() {
    let rules = SAFE.replace(
        "\"portRangeMin\":22,\"portRangeMax\":22",
        "\"portRangeMin\":-0,\"portRangeMax\":65535",
    );
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(&rules),
        response(QUOTE),
        response(GROUP),
        response(&rules),
        http("202 Accepted", ACK),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.no_wait = true;
    assert!(
        floating_client(&server, key_options())
            .create_server(&ctx, input)
            .await
            .is_ok()
    );
    routes(&server, 7);
}

#[tokio::test]
async fn floating_rejects_zero_padded_prefix_before_quote() {
    let rules = SAFE.replace("192.0.2.8/32", "192.0.2.8/032");
    let server = TestServer::start(vec![response(LIST), response(GROUP), response(&rules)]);
    let (ctx, _) = RequestContext::new();
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.error().detail::<OpenLogin>().is_some());
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    routes(&server, 3);
}

#[tokio::test]
async fn exposure_checks_deadline_before_returning_digest() {
    struct Advancing {
        start: tokio::time::Instant,
        calls: AtomicUsize,
    }
    impl crate::Clock for Advancing {
        fn utc_now(&self) -> time::OffsetDateTime {
            time::OffsetDateTime::UNIX_EPOCH
        }
        fn monotonic_now(&self) -> tokio::time::Instant {
            self.start
                + std::time::Duration::from_secs(
                    15 * self.calls.fetch_add(1, Ordering::SeqCst) as u64,
                )
        }
        fn sleep_until(
            &self,
            _: tokio::time::Instant,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }
    }
    let server = TestServer::start(vec![]);
    let options = key_options().with_clock(Arc::new(Advancing {
        start: tokio::time::Instant::now(),
        calls: AtomicUsize::new(0),
    }));
    let client = floating_client(&server, options);
    let (ctx, _) = RequestContext::new();
    let error = client
        .exposure_snapshot(
            &ctx,
            &[],
            true,
            &mut crate::wire::product::evidence::Evidence::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(error.has_class(ErrorClass::DeadlineExceeded));
}

#[tokio::test]
async fn floating_override_keeps_the_rule_limit() {
    let oversized = format!(r#"{{"data":[{}]}}"#, vec!["{}"; 4097].join(","));
    let server = TestServer::start(vec![response(LIST), response(GROUP), response(&oversized)]);
    let (ctx, _) = RequestContext::new();
    let mut input = floating_input();
    input.allow_open_login = true;
    let failure = floating_client(&server, key_options())
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    routes(&server, 3);
}

mod screening;
