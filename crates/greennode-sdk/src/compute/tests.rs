mod actions;
mod groups;
mod paid;
mod ssh_keys;

use super::*;
use crate::wire::test_server::{Response, Server as TestServer};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret};

const CANARY: &str = "toy-secret";

fn response(raw: &str) -> Response {
    http("200 OK", raw)
}

fn http(status: &str, raw: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.as_bytes().to_vec(),
    }
}

fn client(server: &TestServer) -> Client {
    client_with_token(server, CANARY)
}

fn client_with_token(server: &TestServer, token: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(token))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn resize_poll_refuses_a_reflecting_server_snapshot() {
    use super::mutation::Settlement;
    let peer = r#"{"data":{"uuid":"server-1","status":"ACTIVE","flavor":{"flavorId":"flavor-2"},"description":"toy-secret"}}"#;
    let server = TestServer::start(vec![response(peer)]);
    let mut last = Server::default();
    let result = client(&server)
        .settle_server(
            &RequestContext::new().0,
            "server-1",
            Settlement::Resized("flavor-2"),
            &mut last,
            &mut crate::wire::product::evidence::Evidence::new(),
        )
        .await;
    assert!(result.is_err());
    assert!(last.uuid.is_empty());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn terminal_flavor_list_keeps_redaction() {
    let server = TestServer::start(vec![response(
        r#"{"flavors":[{"id":"offer-1","name":"toy-secret"}]}"#,
    )]);
    let result = client(&server)
        .list_flavors(
            &RequestContext::new().0,
            ListFlavorsInput {
                zone_id: String::new(),
                flavor_zone_id: "fz-1".into(),
                name: String::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.items.len(), 1);
    assert!(result.items[0].name == "<redacted>");
}

#[tokio::test]
async fn flavor_zone_discovery_refuses_a_reflected_non_retained_field() {
    let server = TestServer::start(vec![
        response(
            r#"{"flavorZones":[{"id":"flavor-1","zoneId":"zone-1","description":"toy-secret"}]}"#,
        ),
        response(r#"{"flavors":[]}"#),
    ]);
    let result = client(&server)
        .list_flavors(
            &RequestContext::new().0,
            ListFlavorsInput {
                zone_id: "zone-1".into(),
                flavor_zone_id: String::new(),
                name: String::new(),
            },
        )
        .await;
    assert!(result.is_err());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn server_detail_matches_identity_and_redacts_encoded_reflection() {
    let server = TestServer::start(vec![
        response(
            r#"{"data":{"uuid":"server-1","name":"toy%2Dsecret","zone":{"name":"region-a"}}}"#,
        ),
        response(r#"{"data":{"uuid":"server-2"}}"#),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let input = GetServerInput::new("server-1");
    let output = client.get_server(&ctx, input.clone()).await.unwrap();
    assert_eq!(output.server.name, "<redacted>");
    let failure = client.get_server(&ctx, input).await.unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.code(), "InvalidResponse");
    assert_eq!(api.status(), Some(200));
    assert!(server.heads()[0].starts_with(b"GET /v2/project-1/servers/server-1 "));
}

#[tokio::test]
async fn inventory_rejects_bad_pages_and_keeps_provider_identifiers() {
    let server = TestServer::start(vec![
        response(r#"{"listData":[],"page":1,"totalPage":0,"totalItem":0}"#),
        response(
            r#"{"listData":[{"uuid":"server_1"}],"page":2,"pageSize":5,"totalPage":2,"totalItem":6}"#,
        ),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_servers(&ctx, ListServersInput::default())
            .await
            .is_err()
    );
    let output = client
        .list_servers(&ctx, ListServersInput { page: 2, size: 5 })
        .await
        .unwrap();
    assert_eq!(output.items[0].uuid, "server_1");
    assert!(server.heads()[0].starts_with(b"GET /v2/project-1/servers?page=1&size=10000 "));
    assert!(server.heads()[1].starts_with(b"GET /v2/project-1/servers?page=2&size=5 "));
}

#[tokio::test]
async fn path_fields_are_checked_before_a_request() {
    let server = TestServer::start(vec![]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    for id in ["server_1", "server/1", CANARY] {
        let failure = client
            .get_server(&ctx, GetServerInput::new(id))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    }
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn explicitly_selected_disabled_zone_quotes_the_selected_root_disk() {
    macro_rules! fixture {
        ($name:literal) => {
            response(include_str!(concat!(
                "../../tests/fixtures/compute/price-",
                $name,
                ".json"
            )))
        };
    }
    let region = TestServer::start(vec![
        fixture!("zones"),
        fixture!("platforms"),
        fixture!("families"),
        fixture!("flavors"),
        fixture!("disk-groups"),
        fixture!("disks"),
        fixture!("images"),
        fixture!("quote"),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("proj-1".into())
        .with_static_token(Secret::new(CANARY))
        .with_endpoint_overrides(EndpointOverrides {
            vserver: Some(format!("{}/regional/", region.origin)),
            portal: Some(format!("{}/portal/", region.origin)),
            ..EndpointOverrides::default()
        });
    let config = Config::new(options).unwrap();
    config
        .session()
        .set_test_product_root(region.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_flavor_prices(
            &ctx,
            ListFlavorPricesInput {
                zone_id: "zone-1".into(),
                root_disk_type_id: "disk-1".into(),
                ..ListFlavorPricesInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(output.metadata.requests, 8);
    let row = &output.items[0];
    assert_eq!(row.zone_enabled, Some(false));
    assert_eq!(row.price.monthly_price, Some(20.0));
    assert_eq!(row.reference.as_ref().unwrap().root_disk_size_gb, 15);
    assert!(region.heads()[7].starts_with(b"POST /portal/v1/price "));
    let quote: serde_json::Value = serde_json::from_slice(&region.bodies()[7]).unwrap();
    let info = &quote["resourceInfo"];
    assert!(
        info["rootDiskTypeId"] == "disk-1"
            && info["zoneId"] == "zone-1"
            && info["rootDiskSize"] == 15
    );
    assert!(region.heads()[6].starts_with(b"GET /regional/v1/proj-1/images/os?zoneId=zone-1 "));
}

#[tokio::test]
async fn implicit_disabled_zone_keeps_rows_without_reference_requests() {
    let region = TestServer::start(
        ["zones", "platforms", "families", "flavors"]
            .into_iter()
            .map(|name| {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/compute/price-{name}.json"));
                response(&std::fs::read_to_string(path).unwrap())
            })
            .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let output = client(&region)
        .list_flavor_prices(&ctx, ListFlavorPricesInput::default())
        .await
        .unwrap();
    assert_eq!(output.metadata.requests, 4);
    assert_eq!(output.items[0].price.status, "inapplicable");
    assert!(output.items[0].reference.is_none());
}

#[tokio::test]
async fn status_reflection_preserves_permission_denied() {
    let server = TestServer::start(vec![http("403 Forbidden", "{}")]);
    let (ctx, _) = RequestContext::new();
    let error = client_with_token(&server, "403")
        .get_server(&ctx, GetServerInput::new("server-1"))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::PermissionDenied));
    assert_eq!(error.error().api_error().unwrap().status(), None);
}

#[tokio::test]
async fn console_keeps_clean_near_limit_bytes() {
    let mut log = "x".repeat((8 << 20) - 15);
    log.push('\n');
    let body = serde_json::json!({"data":log}).to_string();
    assert!(body.len() <= 8 << 20);
    let server = TestServer::start(vec![response(&body)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_server_console_log(&ctx, GetServerConsoleLogInput::new("server-1"))
        .await
        .unwrap();
    assert!(output.log.expose_secret() == log);
}

#[tokio::test]
async fn typed_envelopes_and_rows_reject_case_variants() {
    let server = TestServer::start(vec![
        response(r#"{"DATA":{"uuid":"server-1"}}"#),
        response(r#"{"data":{"uuid":"server-1","Name":"toy"}}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        let error = client(&server)
            .get_server(&ctx, GetServerInput::new("server-1"))
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    }
}

#[tokio::test]
async fn non_public_envelopes_refuse_incomplete_results() {
    for group in [
        r#"{"flavorZones":[],"totalPage":2}"#,
        r#"{"flavorZones":[],"success":false}"#,
    ] {
        let server = TestServer::start(vec![
            response(r#"{"data":[]}"#),
            response("[]"),
            response(group),
        ]);
        let (ctx, _) = RequestContext::new();
        let error = client(&server)
            .list_flavor_prices(
                &ctx,
                ListFlavorPricesInput {
                    include_non_public: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    }
}

#[tokio::test]
async fn console_http_errors_keep_retryability() {
    for status in [
        "429 Too Many Requests",
        "502 Bad Gateway",
        "503 Service Unavailable",
        "504 Gateway Timeout",
    ] {
        let server = TestServer::start((0..5).map(|_| http(status, "{}")).collect());
        let (ctx, _) = RequestContext::new();
        let error = client(&server)
            .get_server_console_log(&ctx, GetServerConsoleLogInput::new("server-1"))
            .await
            .unwrap_err();
        assert!(error.error().api_error().unwrap().retryable());
    }
}

#[tokio::test]
async fn unknown_root_disk_preserves_invalid_input_identity() {
    let server = TestServer::start(vec![
        response(r#"{"data":[]}"#),
        response("[]"),
        response(r#"{"volumeTypeZones":[]}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .list_flavor_prices(
            &ctx,
            ListFlavorPricesInput {
                root_disk_type_id: "missing-disk".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_identity(ErrorClass::InvalidInput));
}

#[tokio::test]
async fn console_redacts_base64_and_base64url_reflections() {
    let server = TestServer::start(vec![
        response(r#"{"data":"dG95Pz8/Pw=="}"#),
        response(r#"{"data":"dG95Pz8_Pw"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client_with_token(&server, "toy????");
    for _ in 0..2 {
        let output = client
            .get_server_console_log(&ctx, GetServerConsoleLogInput::new("server-1"))
            .await
            .unwrap();
        assert!(output.log.expose_secret() == "<redacted>");
    }
}

fn price_fixture(name: &str) -> Response {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/compute/price-{name}.json"));
    response(&std::fs::read_to_string(path).unwrap())
}

#[tokio::test]
async fn quote_body_is_screened_before_sending() {
    let server = TestServer::start(
        [
            "zones",
            "platforms",
            "families",
            "flavors",
            "disk-groups",
            "disks",
            "images",
            "quote",
        ]
        .into_iter()
        .map(price_fixture)
        .collect(),
    );
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new("encryptionVolume"))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/regional/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .list_flavor_prices(
            &ctx,
            ListFlavorPricesInput {
                zone_id: "zone-1".into(),
                root_disk_type_id: "disk-1".into(),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());
    assert_eq!(server.heads().len(), 7);
}

#[tokio::test]
async fn expanded_flavor_rows_count_against_the_retained_byte_limit() {
    let zone = serde_json::json!({"data":[{"uuid":"zone-1","name":"x".repeat(1 << 20),"isEnabled":false}]}).to_string();
    let flavors = (0..33).map(|i| serde_json::json!({"flavorId":format!("flavor-{i}"),"flavorZoneId":"offer-1","name":"small","cpu":2,"memory":4,"remainingVms":1,"isSoldOut":false})).collect::<Vec<_>>();
    let server = TestServer::start(vec![
        response(&zone),
        price_fixture("platforms"),
        price_fixture("families"),
        response(&serde_json::to_string(&flavors).unwrap()),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .list_flavor_prices(&ctx, ListFlavorPricesInput::default())
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::EstimateLimit));
}

#[tokio::test]
async fn discovery_quote_never_sends_a_body_with_the_refreshed_bearer() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    #[derive(Default)]
    struct Provider {
        invalidated: AtomicBool,
        got_refreshed_token: AtomicBool,
    }
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                let value = if self.invalidated.load(Ordering::Relaxed) {
                    self.got_refreshed_token.store(true, Ordering::Relaxed);
                    "image-1"
                } else {
                    "toy-initial-bearer"
                };
                Ok(crate::Token::new(Secret::new(value), None))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                self.invalidated.store(true, Ordering::Relaxed);
                Ok(())
            })
        }
    }
    let responses = [
        "zones",
        "platforms",
        "families",
        "flavors",
        "disk-groups",
        "disks",
        "images",
    ]
    .into_iter()
    .map(price_fixture)
    .chain([http("401 Unauthorized", "{}"), price_fixture("quote")]);
    let server = TestServer::start(responses.collect());
    let provider = Arc::new(Provider::default());
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_credentials_provider(provider.clone())
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/regional/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let result = Client::new(&config)
        .list_flavor_prices(
            &ctx,
            ListFlavorPricesInput {
                zone_id: "zone-1".into(),
                root_disk_type_id: "disk-1".into(),
                ..Default::default()
            },
        )
        .await;
    let heads = server.heads();
    assert!(provider.got_refreshed_token.load(Ordering::Relaxed));
    assert_eq!(heads.len(), 8);
    assert!(
        heads[..7]
            .iter()
            .all(|head| head.starts_with(b"GET /regional/"))
    );
    assert!(heads[7].starts_with(b"POST /portal/v1/price "));
    assert!(heads.iter().all(|head| {
        !head
            .windows(b"Bearer image-1".len())
            .any(|part| part == b"Bearer image-1")
    }));
    let sent = serde_json::from_slice::<serde_json::Value>(&server.bodies()[7]).unwrap();
    assert!(sent["resourceInfo"]["imageId"] == "image-1");
    assert!(
        result
            .unwrap_err()
            .error()
            .has_class(ErrorClass::InvalidInput)
    );
}

#[tokio::test]
async fn repeated_image_versions_count_against_the_retained_byte_limit() {
    let version = "v1.0.".repeat((1 << 20) / 5);
    let images = serde_json::json!({"images":[{"id":"image-1","imageType":"ubuntu","imageVersion":version,"licence":false,"packageLimit":{"cpu":1,"memory":1,"diskSize":10}}]}).to_string();
    let flavors = (0..33).map(|i| serde_json::json!({"flavorId":format!("flavor-{i}"),"flavorZoneId":"offer-1","name":"small","cpu":2,"memory":4,"remainingVms":1,"isSoldOut":false,"metaData":"{\"imageTypeSupport\":[\"ubuntu\"]}"})).collect::<Vec<_>>();
    let server = TestServer::start(vec![
        price_fixture("zones"),
        price_fixture("platforms"),
        price_fixture("families"),
        response(&serde_json::to_string(&flavors).unwrap()),
        price_fixture("disk-groups"),
        price_fixture("disks"),
        response(&images),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .list_flavor_prices(
            &ctx,
            ListFlavorPricesInput {
                zone_id: "zone-1".into(),
                root_disk_type_id: "disk-1".into(),
                max_requests: 7,
                allow_partial: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::EstimateLimit));
    let partial = error.partial().unwrap();
    let retained = partial
        .items
        .iter()
        .filter(|row| row.reference.is_some())
        .count();
    assert!((1..=31).contains(&retained));
    assert_eq!(partial.items.len(), flavors.len());
    assert!(
        partial.metadata.failures[0]
            .item_key
            .starts_with("zone-1/offer-1/flavor-")
    );
    assert_eq!(server.heads().len(), 7);
}

#[tokio::test]
async fn user_image_page_redacts_a_short_encoding_in_a_projected_id() {
    let server = TestServer::start(vec![response(
        r#"{"listData":[{"uuid":"image-1","projectId":"pre6A6B6Cpost"}],"page":1,"totalPage":1,"totalItem":1}"#,
    )]);
    let page = client_with_token(&server, "jkl")
        .list_user_images(&RequestContext::new().0, ListUserImagesInput::default())
        .await
        .unwrap();
    assert_eq!(server.requests(), 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].uuid, "image-1");
    assert!(page.items[0].project_id == "<redacted>");
}
