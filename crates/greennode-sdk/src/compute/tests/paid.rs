use super::*;

#[tokio::test]
async fn quote_server_actions_only_send_priced_fields_to_portal() {
    let server = TestServer::start(vec![
        response(r#"{"optimumPrice":20}"#),
        response(r#"{"optimumPrice":-15}"#),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let input = CreateServerInput {
        zone_id: "zone-1".into(),
        flavor_id: "flavor-1".into(),
        image_id: "image-1".into(),
        root_disk_size: 20,
        root_disk_type_id: "disk-1".into(),
        root_disk_encryption_type_id: "encryption_1".into(),
        ..CreateServerInput::default()
    };
    assert_eq!(
        client
            .quote_create_server(&ctx, input)
            .await
            .unwrap()
            .optimum_price,
        20.0
    );
    assert_eq!(
        client
            .quote_resize_server(
                &ctx,
                ResizeServerInput {
                    server_id: "server-1".into(),
                    flavor_id: "flavor-2".into(),
                    ..ResizeServerInput::default()
                }
            )
            .await
            .unwrap()
            .optimum_price,
        -15.0
    );
    for head in server.heads() {
        assert!(head.starts_with(b"POST /portal/v1/price "));
    }
    let bodies: Vec<serde_json::Value> = server
        .bodies()
        .iter()
        .map(|body| serde_json::from_slice(body).unwrap())
        .collect();
    assert_eq!(
        bodies,
        vec![
            serde_json::json!({"resourceType":"server","action":"create","resourceInfo":{
                "zoneId":"zone-1","flavorId":"flavor-1","imageId":"image-1","rootDiskSize":20,
                "rootDiskTypeId":"disk-1","encryptionVolume":true,"period":1,"isPoc":false
            }}),
            serde_json::json!({"resourceType":"server","action":"resize","resourceInfo":{
                "serverId":"server-1","flavorId":"flavor-2"
            }})
        ]
    );
}

#[tokio::test]
async fn delete_rejects_reflected_credential_in_server_detail_before_volume_listing() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1","name":"toy%2Dsecret"}}"#),
        response("[]"),
        http("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .delete_server(
            &ctx,
            DeleteServerInput {
                server_id: "server-1".into(),
                no_wait: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn delete_rejects_reflection_in_unknown_server_detail_field() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1","unknown":"toy%2Dsecret"}}"#),
        response("[]"),
        http("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .delete_server(
            &ctx,
            DeleteServerInput {
                server_id: "server-1".into(),
                no_wait: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn delete_rejects_credential_in_reserved_number_key() {
    let server = TestServer::start(vec![
        response(
            r#"{"data":{"uuid":"server-1","unknown":{"$serde_json::private::Number":"123"}}}"#,
        ),
        response("[]"),
        http("202 Accepted", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client_with_token(&server, "$serde_json::private::Number")
        .delete_server(
            &ctx,
            DeleteServerInput {
                server_id: "server-1".into(),
                no_wait: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn delete_does_not_confirm_truncated_not_found_response() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1"}}"#),
        response("[]"),
        http("202 Accepted", ""),
        Response::Http {
            status: "404 Not Found".into(),
            headers: vec![("Content-Length".into(), "10".into())],
            body: Vec::new(),
        },
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .delete_server(
            &ctx,
            DeleteServerInput {
                server_id: "server-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
}

#[tokio::test]
async fn oversized_not_found_retains_status_without_confirming_server_deletion() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1"}}"#),
        response("[]"),
        http("202 Accepted", ""),
        http("404 Not Found", &"x".repeat((8 << 20) + 1)),
    ]);
    let failure = client(&server)
        .delete_server(
            &RequestContext::new().0,
            DeleteServerInput {
                server_id: "server-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(
        failure
            .error()
            .detail::<crate::HttpStatus>()
            .map(|status| status.as_u16()),
        Some(404)
    );
}

#[tokio::test]
async fn deletes_server_and_reports_attached_nonboot_volumes() {
    let server = TestServer::start(vec![
        response(r#"{"data":{"uuid":"server-1","bootVolumeId":"vol-boot"}}"#),
        response(r#"[{"uuid":"vol-boot"},{"uuid":"vol-data"}]"#),
        http("202 Accepted", ""),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let result = client
        .delete_server(
            &ctx,
            DeleteServerInput {
                server_id: "server-1".into(),
                no_wait: true,
                ..DeleteServerInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.kept_volume_ids, vec![String::from("vol-data")]);
    assert!(result.deleted_volume_ids.is_empty());
    assert!(server.heads()[2].starts_with(b"DELETE /v2/project-1/servers/server-1 "));
    let bodies = server.bodies();
    let request: serde_json::Value = serde_json::from_slice(&bodies[2]).unwrap();
    assert_eq!(request, serde_json::json!({"deleteAllVolume":false}));
}

#[tokio::test]
async fn delete_server_guards_each_kept_volume_confirmation() {
    for (note, allowed) in [("ordinary-note", true), (CANARY, false)] {
        let detail =
            serde_json::json!({"data":{"uuid":"vol-data","status":"AVAILABLE","note":note}})
                .to_string();
        let server = TestServer::start(vec![
            response(r#"{"data":{"uuid":"server-1","bootVolumeId":"vol-boot"}}"#),
            response(r#"[{"uuid":"vol-boot"},{"uuid":"vol-data"}]"#),
            http("202 Accepted", ""),
            http("404 Not Found", "{}"),
            response(&detail),
        ]);
        let result = client(&server)
            .delete_server(
                &RequestContext::new().0,
                DeleteServerInput {
                    server_id: "server-1".into(),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(result.is_ok(), allowed);
        match result {
            Ok(result) => assert_eq!(result.kept_volume_ids, ["vol-data"]),
            Err(failure) => {
                assert_eq!(
                    failure.write_dispatch(),
                    crate::WriteDispatch::MayHaveBeenSent
                );
                assert!(
                    failure
                        .partial()
                        .is_none_or(|result| result.kept_volume_ids.is_empty())
                );
            }
        }
        assert_eq!(server.heads().len(), 5);
    }
}

#[tokio::test]
async fn create_server_reports_disk_zone_advice_and_withholds_user_data() {
    let server = TestServer::start(vec![
        response(r#"{"listData":[],"page":1,"pageSize":10,"totalPage":1,"totalItem":0}"#),
        response(r#"{"optimumPrice":20}"#),
        http(
            "400 Bad Request",
            r#"{"code":"DiskTypeUnavailable","message":"cloud-init-toy"}"#,
        ),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let error = Client::new(&config)
        .create_server(
            &ctx,
            CreateServerInput {
                user_data: Some(Secret::new("cloud-init-toy")),
                ssh_key_id: String::new(),
                ..server_order()
            },
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(400));
    assert_eq!(api.code(), "DiskTypeUnavailable");
    assert!(api.message().contains("server message withheld"));
    assert!(api.message().contains("volume.ListVolumeTypes"));
    assert!(!api.message().contains("cloud-init-toy"));
    let heads = server.heads();
    assert!(heads[1].starts_with(b"POST /portal/v1/price "));
    assert!(heads[2].starts_with(b"POST /v2/project-1/servers "));
}

#[tokio::test]
async fn compute_quote_keeps_safe_provider_error_code() {
    let server = TestServer::start(vec![http(
        "500 Internal Server Error",
        r#"{"code":"QuoteUnavailable"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .quote_create_server(&ctx, server_order())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "QuoteUnavailable"
    );
}

#[tokio::test]
async fn negative_floating_component_is_unpriced() {
    let server = TestServer::start(vec![response(
        r#"{"optimumPrice":-1,"originalPrice":20,"discountPrice":0,"discountPercent":0}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let error = server_order_client(&server)
        .quote_create_server(
            &ctx,
            CreateServerInput {
                attach_floating_ip: true,
                ..server_order()
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::Unpriced));
}

#[tokio::test]
async fn floating_quote_requires_totals_present_on_wire() {
    let server = TestServer::start(vec![
        response(r#"{"optimumPrice":20,"originalPrice":20}"#),
        response(r#"{"optimumPrice":20,"originalPrice":20,"discountPrice":0,"discountPercent":0}"#),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let input = CreateServerInput {
        zone_id: "zone-1".into(),
        flavor_id: "flavor-1".into(),
        image_id: "image-1".into(),
        root_disk_size: 20,
        root_disk_type_id: "disk-1".into(),
        attach_floating_ip: true,
        ..CreateServerInput::default()
    };
    let invalid = client
        .quote_create_server(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(invalid.error().api_error().unwrap().status(), Some(200));
    let quote = client.quote_create_server(&ctx, input).await.unwrap();
    assert_eq!(quote.optimum_price, 20.0);
    assert_eq!(quote.properties[0].name, "FLOATING IP");
}

fn server_order() -> CreateServerInput {
    CreateServerInput {
        name: "example".into(),
        zone_id: "zone-1".into(),
        flavor_id: "flavor-1".into(),
        image_id: "image-1".into(),
        vpc_id: "vpc-1".into(),
        subnet_id: "subnet-1".into(),
        security_group_ids: vec!["group-1".into()],
        root_disk_size: 20,
        root_disk_type_id: "disk-1".into(),
        max_price: 20.0,
        ssh_key_id: "key-1".into(),
        ..CreateServerInput::default()
    }
}

fn server_order_client(server: &TestServer) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

fn inventory() -> Response {
    response(include_str!(
        "../../../tests/fixtures/compute/server-order-inventory.json"
    ))
}

#[tokio::test]
async fn create_gateway_failure_keeps_reason_in_recovery_advice() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("502 Bad Gateway", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .create_server(&ctx, server_order())
        .await
        .unwrap_err();
    let advice = failure
        .error()
        .detail::<crate::error::RecoveryAdvice>()
        .unwrap()
        .message();
    assert!(advice.contains("Bad Gateway"));
    assert!(advice.contains("match the name exactly"));
}

#[tokio::test]
async fn create_recovery_advice_does_not_join_into_user_data() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("502 Bad Gateway", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let secret = "withheld: it may quote user data; create may";
    let failure = server_order_client(&server)
        .create_server(
            &ctx,
            CreateServerInput {
                user_data: Some(Secret::new(secret)),
                ssh_key_id: String::new(),
                ..server_order()
            },
        )
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .detail::<crate::error::RecoveryAdvice>()
            .is_none_or(|advice| !advice.message().contains(secret))
    );
}

#[tokio::test]
async fn settlement_error_exposes_confirmation_api_status() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("202 Accepted", r#"{"data":{"uuid":"server-1"}}"#),
        http("403 Forbidden", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .create_server(&ctx, server_order())
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), Some(403));
}

#[tokio::test]
async fn confirmation_bearer_is_redacted_from_create_partial() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Provider(AtomicUsize);
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                let token = if self.0.fetch_add(1, Ordering::SeqCst) >= 3 {
                    "server-1"
                } else {
                    "initial-token"
                };
                Ok(crate::Token::new(Secret::new(token), None))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("202 Accepted", r#"{"data":{"uuid":"server-1"}}"#),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_credentials_provider(Arc::new(Provider(AtomicUsize::new(0))))
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
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .create_server(&ctx, server_order())
        .await
        .unwrap_err();
    assert!(
        failure
            .partial()
            .is_none_or(|partial| partial.server.uuid == "<redacted>")
    );
    assert_eq!(server.heads().len(), 3);
}

#[tokio::test]
async fn create_server_never_polls_an_ambiguous_acknowledgement() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http(
            "202 Accepted",
            r#"{"data":{"uuid":"server-1","UUID":"server-2"}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .create_server(&ctx, server_order())
        .await
        .unwrap_err();
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    assert_eq!(server.heads().len(), 3);
    assert!(server.heads()[1].starts_with(b"POST /portal/v1/price "));
}

#[tokio::test]
async fn create_server_retains_the_accepted_partial_when_the_settlement_read_reflects() {
    let server = TestServer::start(vec![
        inventory(),
        response(r#"{"optimumPrice":20}"#),
        http("202 Accepted", r#"{"data":{"uuid":"server-1"}}"#),
        response(r#"{"data":{"uuid":"server-1","status":"ERROR","name":"toy%2Dsecret"}}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .create_server(&ctx, server_order())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    let output = failure.partial().unwrap();
    assert_eq!(output.server.name, server_order().name);
    assert!(output.server.status.is_empty());
    assert_eq!(output.server.uuid, "server-1");
    assert_eq!(output.monthly_price, 20.0);
    assert!(server.heads()[1].starts_with(b"POST /portal/v1/price "));
    assert!(server.heads()[3].starts_with(b"GET /v2/project-1/servers/server-1 "));
}

#[tokio::test]
async fn create_server_refuses_floating_attachment_before_network() {
    let server = TestServer::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let failure = server_order_client(&server)
        .create_server(
            &ctx,
            CreateServerInput {
                attach_floating_ip: true,
                ..server_order()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn compute_quote_keeps_clean_peer_message_and_withholds_a_reflected_one() {
    let (ctx, _) = RequestContext::new();
    let clean = TestServer::start(vec![http(
        "400 Bad Request",
        r#"{"code":"QuoteRefused","message":"flavor is not orderable"}"#,
    )]);
    let failure = server_order_client(&clean)
        .quote_create_server(&ctx, server_order())
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.code(), "QuoteRefused");
    assert_eq!(api.message(), "flavor is not orderable");
    let reflected = TestServer::start(vec![http(
        "400 Bad Request",
        &format!(r#"{{"code":"QuoteRefused","message":"bad {CANARY}"}}"#),
    )]);
    let failure = server_order_client(&reflected)
        .quote_create_server(&ctx, server_order())
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.message(), "Bad Request");
}
