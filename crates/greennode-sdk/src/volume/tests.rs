mod writes;

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

const TOKEN: &str = "synthetic-volume-bearer";
const VOLUMES: &str = include_str!("../../tests/fixtures/volume/list_volumes.json");
const VOLUME: &str = include_str!("../../tests/fixtures/volume/get_volume.json");
const BACKENDS: &str = include_str!("../../tests/fixtures/volume/snapshot_backends.json");
const POLICIES: &str = include_str!("../../tests/fixtures/volume/snapshot_policies.json");

fn response(body: &str) -> Response {
    Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server, region: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_static_token(Secret::new(TOKEN))
            .with_region(region.into())
            .with_project_id("project-1".into())
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/server/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                vserver_backup: Some(format!("{}/backup/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn inventory_uses_v2_routes_and_checks_detail_identity() {
    let server = Server::start(vec![response(VOLUMES), response(VOLUME), response(VOLUME)]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let volumes = client
        .list_volumes(&ctx, ListVolumesInput::default())
        .await
        .unwrap();
    assert_eq!(volumes.items[0].uuid, "volume-1");
    let volume = client
        .get_volume(
            &ctx,
            GetVolumeInput {
                volume_id: "volume-1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(volume.volume.status, "AVAILABLE");
    let error = client
        .get_volume(
            &ctx,
            GetVolumeInput {
                volume_id: "volume-2".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /server/v2/project-1/volumes?name=&page=1&size=10000 "));
    assert!(heads[1].starts_with(b"GET /server/v2/project-1/volumes/volume-1 "));
}

#[tokio::test]
async fn inventory_refuses_encoded_reflections() {
    let body = VOLUMES.replace("<name>", "synthetic%2dvolume%2dbearer");
    let server = Server::start(vec![response(&body)]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, "hcm-3")
        .list_volumes(&ctx, ListVolumesInput::default())
        .await
        .unwrap_err();
    assert!(error.partial().is_none());
    assert!(!format!("{error:?}").contains(TOKEN));
}

#[tokio::test]
async fn snapshot_backends_and_policies_use_han_backup_gateway() {
    let server = Server::start(vec![response(BACKENDS), response(POLICIES)]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let backends = client
        .list_snapshot_backends(
            &ctx,
            ListSnapshotBackendsInput {
                name: "HAN-01".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(backends.items[0].id, "backend-1");
    let policies = client
        .list_snapshot_policies(
            &ctx,
            ListSnapshotPoliciesInput {
                backend_id: "backend-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(policies.items[0].id, "policy-1");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /backup/v1/backends?backend=HAN-01 "));
    assert!(heads[1].starts_with(
        b"GET /backup/v1/snapshot-policies?backendId=backend-1&page=1&projectId=project-1&size=10 "
    ));
}

#[tokio::test]
async fn explicitly_selected_disabled_zone_is_quoted_at_offering_minimum() {
    let server = Server::start(vec![
        response(r#"{"data":[{"uuid":"zone-1","name":"Disabled","isEnabled":false}]}"#),
        response(r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#),
        response(r#"[{"key":"synthetic_encryption"}]"#),
        response(include_str!(
            "../../tests/fixtures/volume/discovery_types.json"
        )),
        response(include_str!(
            "../../tests/fixtures/volume/discovery_quote_plain.json"
        )),
    ]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_volume_prices(
            &ctx,
            ListVolumePricesInput {
                zone_id: "zone-1".into(),
                volume_type_id: "vtype-1".into(),
                encryption: "none".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let row = &output.items[0];
    assert_eq!(row.zone_enabled, Some(false));
    assert_eq!(row.size_gb, row.volume_type.min_size);
    assert_eq!(row.price.status, "quoted");
    assert_eq!(output.metadata.requests, 5);
    let heads = server.heads();
    assert!(
        heads[..4]
            .iter()
            .all(|head| head.starts_with(b"GET /server/v1/"))
    );
    assert!(heads[4].starts_with(b"POST /portal/v1/price "));
}

#[tokio::test]
async fn quote_only_type_ids_accept_underscores_without_relaxing_path_ids() {
    let server = Server::start(vec![
        response(r#"{"data":[{"uuid":"zone_1","isEnabled":true}]}"#),
        response(r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#),
        response("[]"),
        response(r#"{"volumeTypes":[{"id":"type_1","name":"SSD","minSize":1,"maxSize":10}]}"#),
        response(r#"{"optimumPrice":100}"#),
    ]);
    let client = client(&server, "hcm-3");
    let (ctx, _) = RequestContext::new();
    let path_error = client
        .get_volume_type(
            &ctx,
            GetVolumeTypeInput {
                volume_type_id: "type_1".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(path_error.error().class(), crate::ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
    let prices = client
        .list_volume_prices(
            &ctx,
            ListVolumePricesInput {
                volume_type_id: "type_1".into(),
                encryption: "none".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(prices.items[0].volume_type.id, "type_1");
    assert_eq!(prices.items[0].price.monthly_price, Some(100.0));
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn quote_refuses_configured_bearer_in_body_before_sending() {
    let server = Server::start(vec![response(r#"{"optimumPrice":100}"#)]);
    let client = client(&server, "hcm-3");
    let (ctx, _) = RequestContext::new();
    let error = client
        .quote_create_volume(
            &ctx,
            CreateVolumeInput {
                zone_id: "synthetic%2dvolume%2dbearer".into(),
                volume_type_id: "type-1".into(),
                size: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), crate::ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn discovery_reports_bounds_without_quoting_inapplicable_sizes() {
    let server = Server::start(vec![
        response(r#"{"data":[{"uuid":"zone-1","isEnabled":true}]}"#),
        response(r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#),
        response("[]"),
        response(r#"{"volumeTypes":[{"id":"type-1","name":"SSD","minSize":10,"maxSize":100}]}"#),
    ]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let output = client
        .list_volume_prices(
            &ctx,
            ListVolumePricesInput {
                size: 101,
                encryption: "none".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let row = &output.items[0];
    assert_eq!(
        (row.volume_type.min_size, row.volume_type.max_size),
        (10, 100)
    );
    assert_eq!(row.price.status, "inapplicable");
    assert_eq!(row.price.monthly_price, None);
    assert!(
        output
            .metadata
            .notes
            .iter()
            .any(|note| note.scope == "zone-1/type-1" && note.topic == "inapplicable")
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn volume_detail_redacts_reflected_display_text() {
    let body = VOLUME.replace("<name>", "synthetic%2dvolume%2dbearer");
    let server = Server::start(vec![response(&body)]);
    let (ctx, _) = RequestContext::new();
    let volume = client(&server, "hcm-3")
        .get_volume(
            &ctx,
            GetVolumeInput {
                volume_id: "volume-1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(volume.volume.name, "<redacted>");
    assert_eq!(volume.volume.uuid, "volume-1");
}

#[tokio::test]
async fn volume_inventory_normalizes_go_strings_and_refuses_duplicate_fields() {
    let clean = r#"{"listData":[{"uuid":"volume_1","name":"\ud800","bootIndex":-0}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#;
    let conflicting = clean.replace("\"bootIndex\":-0", "\"BootIndex\":1,\"bootIndex\":-0");
    let server = Server::start(vec![response(clean), response(&conflicting)]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "hcm-3");
    let output = client
        .list_volumes(&ctx, ListVolumesInput::default())
        .await
        .unwrap();
    assert_eq!(output.items[0].uuid, "volume_1");
    assert_eq!(output.items[0].name, "\u{fffd}");
    assert_eq!(output.items[0].boot_index, 0);
    let error = client
        .list_volumes(&ctx, ListVolumesInput::default())
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
}

#[tokio::test]
async fn resize_quote_refuses_a_reflected_type_from_volume_detail() {
    let body = VOLUME.replace("type-1", "synthetic%2dvolume%2dbearer");
    let server = Server::start(vec![response(&body), response(r#"{"optimumPrice":100}"#)]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, "hcm-3")
        .quote_resize_volume(
            &ctx,
            ResizeVolumeInput {
                volume_id: "volume-1".into(),
                size: 1000,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), crate::ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn quote_refuses_body_against_provider_token_and_refreshed_token() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Provider(AtomicBool);
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                let value = if self.0.load(Ordering::Relaxed) {
                    "toy-refresh-bearer"
                } else {
                    "toy-first-bearer"
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
                self.0.store(true, Ordering::Relaxed);
                Ok(())
            })
        }
    }
    for (selector, sent) in [
        ("toy%2dfirst%2dbearer", 0),
        ("toy%2drefresh%2dbearer", 1),
        ("toy%25252dvalue", 0),
    ] {
        let server = Server::start(vec![
            Response::Http {
                status: "401 Unauthorized".into(),
                headers: vec![("Content-Length".into(), "2".into())],
                body: b"{}".to_vec(),
            },
            response(r#"{"optimumPrice":5}"#),
        ]);
        let config = Config::new(
            ConfigOptions::default()
                .with_credentials_provider(Arc::new(Provider(AtomicBool::new(false))))
                .with_region("hcm-3".into())
                .with_endpoint_overrides(EndpointOverrides {
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
            .quote_create_volume(
                &ctx,
                CreateVolumeInput {
                    volume_type_id: "type-1".into(),
                    zone_id: selector.into(),
                    size: 1,
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(server.requests(), sent);
        assert_eq!(
            result.unwrap_err().error().class(),
            crate::ErrorClass::InvalidInput
        );
    }
}

#[tokio::test]
async fn reflected_discovery_quote_status_is_refused_before_retry() {
    let reflected = format!(r#"{{"message":"{}"}}"#, TOKEN.replace('-', "%2d"));
    let server = Server::start(vec![
        response(r#"{"data":[{"uuid":"zone-1","isEnabled":true}]}"#),
        response(r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#),
        response("[]"),
        response(r#"{"volumeTypes":[{"id":"type-1","name":"SSD","minSize":1,"maxSize":10}]}"#),
        Response::Http {
            status: "503 Service Unavailable".into(),
            headers: vec![("Content-Length".into(), reflected.len().to_string())],
            body: reflected.into_bytes(),
        },
        response(r#"{"optimumPrice":5}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, "hcm-3")
        .list_volume_prices(
            &ctx,
            ListVolumePricesInput {
                encryption: "none".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(error.error().api_error().unwrap().status(), Some(503));
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn resize_quote_refuses_invalid_returned_type_before_post() {
    let (ctx, _) = RequestContext::new();
    for kind in ["", "bad/type", "bad.type"] {
        let detail = VOLUME.replace("type-1", kind);
        let server = Server::start(vec![response(&detail), response(r#"{"optimumPrice":5}"#)]);
        let result = client(&server, "hcm-3")
            .quote_resize_volume(
                &ctx,
                ResizeVolumeInput {
                    volume_id: "volume-1".into(),
                    size: 100,
                    ..Default::default()
                },
            )
            .await;
        assert!(result.is_err());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn discovery_distinguishes_catalog_schema_and_http_operations() {
    let (ctx, _) = RequestContext::new();
    for (answer, operation, status) in [
        (
            response(r#"{"volumeTypeZones":null}"#),
            "volume.ListVolumePrices",
            200,
        ),
        (
            Response::Http {
                status: "400 Bad Request".into(),
                headers: vec![],
                body: b"{}".to_vec(),
            },
            "volume.ListVolumeTypeZones",
            400,
        ),
    ] {
        let server = Server::start(vec![
            response(r#"{"data":[{"uuid":"zone-1","isEnabled":true}]}"#),
            answer,
        ]);
        let error = client(&server, "hcm-3")
            .list_volume_prices(&ctx, ListVolumePricesInput::default())
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().operation(), operation);
        assert_eq!(error.error().api_error().unwrap().status(), Some(status));
    }
}

#[tokio::test]
async fn null_encryption_wrapper_precedes_later_arrays() {
    let server = Server::start(vec![response(
        r#"{"encryptionTypes":null,"data":["toy-encryption-a"]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let rows = client(&server, "hcm-3")
        .list_encryption_types(&ctx, ListEncryptionTypesInput)
        .await
        .unwrap();
    assert!(rows.items.is_empty());
}

#[tokio::test]
async fn volume_quote_keeps_the_peer_refusal_but_discovery_prices_are_status_only() {
    let refused = r#"{"code":"QuoteRefused","message":"size is not orderable"}"#;
    let refusal = || Response::Http {
        status: "400 Bad Request".into(),
        headers: vec![("Content-Length".into(), refused.len().to_string())],
        body: refused.as_bytes().to_vec(),
    };
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response(VOLUME), refusal()]);
    let error = client(&server, "hcm-3")
        .quote_resize_volume(
            &ctx,
            ResizeVolumeInput {
                volume_id: "volume-1".into(),
                size: 1000,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.code(), "QuoteRefused");
    assert_eq!(api.message(), "size is not orderable");
    let server = Server::start(vec![
        response(r#"{"data":[{"uuid":"zone_1","isEnabled":true}]}"#),
        response(r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#),
        response("[]"),
        response(r#"{"volumeTypes":[{"id":"type_1","name":"SSD","minSize":1,"maxSize":10}]}"#),
        refusal(),
    ]);
    let error = client(&server, "hcm-3")
        .list_volume_prices(
            &ctx,
            ListVolumePricesInput {
                volume_type_id: "type_1".into(),
                encryption: "none".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.message(), "Bad Request");
    assert_ne!(api.code(), "QuoteRefused");
}
