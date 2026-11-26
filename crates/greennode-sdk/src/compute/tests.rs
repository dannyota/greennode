use super::*;
use crate::wire::test_server::{Response, Server as TestServer};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret};

const CANARY: &str = "toy-secret";

fn response(raw: &str) -> Response {
    Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.as_bytes().to_vec(),
    }
}

fn client(server: &TestServer) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
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
