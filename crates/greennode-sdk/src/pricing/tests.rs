use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret, WriteDispatch};
use serde_json::{Value, json};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../tests/fixtures/pricing/", $name, ".json"))
    };
}

const CANARY: &str = "estimate-canary";
const OK: &str = "200 OK";

fn response(status: &str, raw: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.as_bytes().to_vec(),
    }
}

fn client(server: &Server) -> Client {
    client_with_project(server, true)
}

fn client_with_project(server: &Server, known: bool) -> Client {
    let endpoint = format!("{}/", server.origin);
    let mut options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(CANARY))
        .with_endpoint_overrides(EndpointOverrides {
            portal: Some(endpoint.clone()),
            storage: Some(endpoint.clone()),
            vserver: Some(endpoint),
            ..EndpointOverrides::default()
        });
    if known {
        options = options.with_project_id("project-1".into());
    }
    let config = Config::new(options).unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn quote_defaults_to_create_and_preserves_signed_resize_credit() {
    let server = Server::start(vec![
        response(OK, fixture!("GetQuoteSnapshot")),
        response(OK, fixture!("GetQuotePublicVIP")),
        response(OK, fixture!("GetQuoteServerResizeCredit")),
        response(OK, fixture!("GetQuoteServerResizeCredit")),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let price = client
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SNAPSHOT))
        .await
        .unwrap();
    assert_eq!(price.optimum_price, 5040.0);
    let mut ip = GetQuoteInput::new(RESOURCE_PUBLIC_VIP);
    ip.resource_info = Some([("type".into(), "public".into())].into_iter().collect());
    assert!(client.get_quote(&ctx, ip).await.is_ok());
    let mut resize = GetQuoteInput::new(RESOURCE_SERVER);
    resize.action = ACTION_RESIZE.into();
    assert!(client.get_quote(&ctx, resize).await.unwrap().optimum_price < 0.0);
    let failure = client
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"POST /v1/price "))
    );
    let bodies = server.bodies();
    assert_eq!(
        serde_json::from_slice::<Value>(&bodies[0]).unwrap(),
        json!({"resourceType":"snapshot","action":"create"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&bodies[1]).unwrap()["resourceInfo"]["type"],
        "public"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&bodies[2]).unwrap()["action"],
        "resize"
    );
}

#[tokio::test]
async fn quote_rejects_ambiguous_success_and_preserves_observed_status() {
    for (status, body, expected) in [
        (OK, "{", 200),
        (OK, "[]", 200),
        (OK, r#"{"optimumPrice":null}"#, 200),
        (OK, r#"{"optimumPrice":1,"OptimumPrice":2}"#, 200),
        (OK, r#"{"optimumPrice":1,"estimate-canary":"hidden"}"#, 200),
        ("302 Found", "{}", 302),
    ] {
        let server = Server::start(vec![response(status, body)]);
        let (ctx, _) = RequestContext::new();
        let error = client(&server)
            .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(api.status(), Some(expected));
        assert_eq!(api.code(), "InvalidResponse");
        assert!(!error.error().to_string().contains(CANARY));
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn reflects_early_error_before_retries_and_preserves_unknown_survivor() {
    let server = Server::start(vec![response(
        "503 Service Unavailable",
        r#"{"estimate-canary":"hidden"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let setup = EstimateInput {
        allow_partial: true,
        public_ips: vec![
            EstimatePublicIp::new("good", "reservation", "", 1),
            EstimatePublicIp::new("bad", "public-vip", "public", 2),
        ],
        ..EstimateInput::default()
    };
    let error = client(&server).estimate(&ctx, setup).await.unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::DiscoveryIncomplete);
    let partial = error.partial().unwrap();
    assert_eq!(partial.items[0].price.status, "unknown");
    assert_eq!(partial.metadata.failures[0].code, "InvalidResponse");
    assert_eq!(partial.metadata.failures[0].status, 503);
    assert!(!serde_json::to_string(partial).unwrap().contains(CANARY));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn estimates_explicit_disabled_zone_and_keeps_disk_quantity_independent() {
    let server = Server::start(vec![
        response(OK, r#"{"data":[{"uuid":"HCM03-1C","isEnabled":false}]}"#),
        response(OK, fixture!("list_volume_type_zones")),
        response(OK, fixture!("discovery_types")),
        response(OK, fixture!("discovery_encryption_types")),
        response(OK, fixture!("discovery_os_images")),
        response(OK, fixture!("estimate_quote_server_encrypted_data")),
        response(OK, fixture!("discovery_quote_plain")),
    ]);
    let mut server_input =
        EstimateServer::new("web", "HCM03-1C", "flav-1", "img-5", 2, 20, "vtype-1");
    server_input.root_disk_encryption_type_id = "toy-encryption-b".into();
    server_input.data_disk_encryption_type_id = "toy-encryption-b".into();
    server_input.data_disk_size = 1;
    server_input.data_disk_type_id = "vtype-1".into();
    let mut volume = EstimateVolume::new("extra", "HCM03-1C", "vtype-1", 3, 1);
    volume.server_key = "web".into();
    let (ctx, _) = RequestContext::new();
    let setup = EstimateInput {
        servers: vec![server_input],
        volumes: vec![volume],
        ..EstimateInput::default()
    };
    let output = client(&server).estimate(&ctx, setup).await.unwrap();
    assert_eq!(output.items[0].monthly_amount, Some(872280.0));
    assert_eq!(output.items[0].breakdown.len(), 4);
    assert_eq!(
        output.items[1].monthly_amount,
        output.items[1].price.monthly_price.map(|value| value * 3.0)
    );
    assert!(output.total.requested_complete);
    assert_eq!(output.metadata.requests, 7);
    let heads = server.heads();
    for (head, path) in heads.iter().zip([
        "GET /v1/project-1/zones ",
        "GET /v1/project-1/volume_type_zones ",
        "GET /v1/project-1/volume-zone-1/volume_types ",
        "GET /v1/project-1/volumes/encryption_types ",
        "GET /v1/project-1/images/os?zoneId=HCM03-1C ",
        "POST /v1/price ",
        "POST /v1/price ",
    ]) {
        assert!(head.starts_with(path.as_bytes()));
    }
    let bodies = server.bodies();
    let server_quote: Value = serde_json::from_slice(&bodies[5]).unwrap();
    assert!(
        server_quote
            == json!({"resourceType":"server","action":"create","resourceInfo":{"zoneId":"HCM03-1C","flavorId":"flav-1","imageId":"img-5","rootDiskSize":20,"rootDiskTypeId":"vtype-1","dataDiskSize":1,"dataDiskTypeId":"vtype-1","encryptionVolume":true,"period":1,"isPoc":false}})
    );
}

#[tokio::test]
async fn resolves_storage_region_by_name_and_id_and_quotes_once() {
    let server = Server::start(vec![
        response(
            OK,
            r#"{"success":true,"datas":[{"regionId":"region-2","regionName":"DAD01"}]}"#,
        ),
        response(OK, fixture!("discovery_project_types")),
        response(OK, fixture!("discovery_purchase_types")),
        response(OK, fixture!("discovery_config_min_quota")),
        response(OK, fixture!("discovery_config_max_quota")),
        response(OK, fixture!("discovery_config_max_projects")),
        response(OK, fixture!("discovery_config_iam_checkout")),
        response(OK, fixture!("discovery_quote_gold")),
    ]);
    let mut first = EstimateStorage::new("first", "Gold", 1, 30);
    first.region = "dad01".into();
    let mut second = EstimateStorage::new("second", "Gold", 2, 30);
    second.region = "region-2".into();
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .estimate(
            &ctx,
            EstimateInput {
                storage: vec![first, second],
                ..EstimateInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(output.metadata.requests, 8);
    assert!(output.items.iter().all(|item| item.region == "DAD01"));
    assert_eq!(
        output.total.requested_monthly_total,
        output.items[0].monthly_amount.map(|amount| amount * 3.0)
    );
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /internal/v1/regions "));
    assert!(heads[7].starts_with(b"POST /billing-api/v2/price?region_id=region-2 "));
    for head in &heads[1..] {
        let head = String::from_utf8_lossy(head).to_ascii_lowercase();
        assert!(head.contains("region: region-2\r\n"));
        assert!(head.contains("region_id: region-2\r\n"));
    }
    let quote: Value = serde_json::from_slice(&server.bodies()[7]).unwrap();
    assert!(
        quote
            == json!({"resourceType":"object_storage","action":"create","resourceInfo":{"quota":30,"purchaseTypeId":4,"projectType":1}})
    );
}

#[tokio::test]
async fn preflight_refuses_quote_phase_when_distinct_requests_exceed_budget() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .estimate(
            &ctx,
            EstimateInput {
                max_requests: 1,
                public_ips: vec![
                    EstimatePublicIp::new("a", "public-vip", "public", 1),
                    EstimatePublicIp::new("b", "public-vip", "public-vm", 5),
                ],
                ..EstimateInput::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::EstimateLimit);
    assert!(
        failure
            .error()
            .api_error()
            .unwrap()
            .message()
            .contains("requires 2 requests")
    );
    assert_eq!(server.requests(), 0);
}

#[test]
fn estimate_decoder_preserves_captured_breakdown_and_evaluation_basis() {
    for (raw, amount, count) in [
        (fixture!("estimate_quote_server_data"), 351000.0, 3),
        (
            fixture!("estimate_quote_server_encrypted_root"),
            432940.0,
            3,
        ),
    ] {
        let raw = serde_json::from_str(raw).unwrap();
        let quote =
            crate::quote::request::decode::estimate("pricing.Estimate", &raw, false).unwrap();
        assert_eq!(quote.price.monthly_price, Some(amount));
        assert_eq!(quote.breakdown.len(), count);
    }
    let flagged = serde_json::from_str(fixture!("estimate_quote_public_flagged")).unwrap();
    assert!(crate::quote::request::decode::estimate("pricing.Estimate", &flagged, false).is_ok());
}

#[tokio::test]
async fn cached_project_inventory_does_not_charge_a_second_estimate() {
    let server = Server::start(vec![
        response(
            OK,
            r#"{"projects":[{"projectId":"project-1","region":"hcm-3"}]}"#,
        ),
        response(OK, r#"{"optimumPrice":1}"#),
        response(OK, r#"{"optimumPrice":2}"#),
    ]);
    let client = client_with_project(&server, false);
    let (ctx, _) = RequestContext::new();
    let setup = |max_requests| EstimateInput {
        max_requests,
        public_ips: vec![EstimatePublicIp::new("ip", "public-vip", "public", 1)],
        ..EstimateInput::default()
    };
    let first = client.estimate(&ctx, setup(2)).await.unwrap();
    let second = client.estimate(&ctx, setup(1)).await.unwrap();
    assert_eq!(first.metadata.requests, 2);
    assert_eq!(second.metadata.requests, 1);
    assert_eq!(first.total.requested_monthly_total, Some(1.0));
    assert_eq!(second.total.requested_monthly_total, Some(2.0));
    assert_eq!(server.requests(), 3);
    assert!(server.heads()[0].starts_with(b"GET /v1/projects "));
}

#[tokio::test]
async fn ambiguous_storage_alias_and_unlisted_zone_refuse_before_quote() {
    let (ctx, _) = RequestContext::new();
    let storage = Server::start(vec![response(
        OK,
        r#"{"success":true,"datas":[{"regionId":"region-2","regionName":"DAD01"},{"regionId":"region-3","regionName":"dad01"}]}"#,
    )]);
    let mut group = EstimateStorage::new("objects", "Gold", 1, 30);
    group.region = "DAD01".into();
    let error = client(&storage)
        .estimate(
            &ctx,
            EstimateInput {
                storage: vec![group],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(storage.requests(), 1);
    let regional = Server::start(vec![response(
        OK,
        r#"{"data":[{"uuid":"HCM03-1C","isEnabled":false}]}"#,
    )]);
    let error = client(&regional)
        .estimate(
            &ctx,
            EstimateInput {
                volumes: vec![EstimateVolume::new("disk", "absent", "vtype-1", 1, 1)],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(regional.requests(), 1);
}

#[tokio::test]
async fn multiplication_overflow_preserves_unknown_rows_without_a_subtotal() {
    let server = Server::start(vec![response(OK, r#"{"optimumPrice":1e308}"#)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .estimate(
            &ctx,
            EstimateInput {
                allow_partial: true,
                public_ips: vec![
                    EstimatePublicIp::new("large", "public-vip", "public", 2),
                    EstimatePublicIp::new("unknown", "reservation", "", 1),
                ],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::DiscoveryIncomplete));
    assert!(failure.error().has_class(ErrorClass::EstimateOverflow));
    assert_eq!(
        failure
            .error()
            .detail::<EstimateArithmeticError>()
            .unwrap()
            .stage,
        "multiplication"
    );
    let partial = failure.partial().unwrap();
    assert_eq!(partial.items[0].price.monthly_price, None);
    assert_eq!(partial.total.known_monthly_subtotal, None);
    assert_eq!(
        partial.metadata.failures[0].code,
        "EstimateMultiplicationOverflow"
    );
}
