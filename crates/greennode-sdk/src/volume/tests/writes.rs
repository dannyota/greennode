use super::{client, response};
use crate::volume::*;
use crate::wire::test_server::{Response, Server};
use crate::{ErrorClass, RequestContext, WriteDispatch};

const AVAILABLE: &str = r#"{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"AVAILABLE","volumeTypeId":"type-1","serverId":null,"serverIdList":[]}}"#;
const IN_USE: &str = r#"{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"IN-USE","volumeTypeId":"type-1","serverId":"server-1","serverIdList":["server-1"]}}"#;
const EMPTY: &str = r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#;
const QUOTE: &str = r#"{"optimumPrice":3200}"#;
const ACCEPTED: &str = "202 Accepted";

fn reply(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn create() -> CreateVolumeInput {
    CreateVolumeInput {
        name: "toy-volume".into(),
        zone_id: "zone-1".into(),
        volume_type_id: "type-1".into(),
        size: 1,
        max_price: 3200.0,
        ..Default::default()
    }
}

fn resize() -> ResizeVolumeInput {
    ResizeVolumeInput {
        volume_id: "volume-1".into(),
        size: 2,
        max_price: 3200.0,
        ..Default::default()
    }
}

#[tokio::test]
async fn repeatable_attach_retries_then_confirms_in_use_by_target() {
    let server = Server::start(vec![
        response(AVAILABLE),
        reply("503 Service Unavailable", "{}"),
        reply(ACCEPTED, ""),
        response(IN_USE),
    ]);
    let (ctx, _) = RequestContext::new();
    let out = client(&server, "han-1")
        .attach_volume(
            &ctx,
            AttachVolumeInput {
                volume_id: "volume-1".into(),
                server_id: "server-1".into(),
                no_wait: false,
            },
        )
        .await
        .unwrap();
    assert!(out.changed);
    assert_eq!(out.volume.server_id, "server-1");
    let heads = server.heads();
    assert!(heads[1..=2].iter().all(|head| {
        head.starts_with(b"PUT /server/v2/project-1/volumes/volume-1/servers/server-1/attach ")
    }));
    let bodies = server.bodies();
    assert!(bodies[1..=2].iter().all(|body| body == b"{}"));
    assert!(heads[3].starts_with(b"GET /server/v2/project-1/volumes/volume-1 "));
}

#[tokio::test]
async fn detach_checks_boot_volume_even_when_running_is_allowed() {
    let (ctx, _) = RequestContext::new();
    for boot in [
        r#"{"data":{"status":"ACTIVE","bootVolumeId":"volume-1"}}"#,
        r#"{"data":{"status":"ACTIVE"}}"#,
    ] {
        let server = Server::start(vec![response(IN_USE), response(boot)]);
        let failure = client(&server, "hcm-3")
            .detach_volume(
                &ctx,
                DetachVolumeInput {
                    volume_id: "volume-1".into(),
                    server_id: "server-1".into(),
                    allow_running: true,
                    no_wait: true,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            failure.error().detail::<WriteRefusal>(),
            Some(&WriteRefusal::BootVolume)
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
    }
    let server = Server::start(vec![
        response(IN_USE),
        response(r#"{"data":{"status":"ACTIVE","bootVolumeId":"boot-1"}}"#),
        reply(ACCEPTED, ""),
        response(AVAILABLE),
    ]);
    let detached = client(&server, "hcm-3")
        .detach_volume(
            &ctx,
            DetachVolumeInput {
                volume_id: "volume-1".into(),
                server_id: "server-1".into(),
                allow_running: true,
                no_wait: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(detached.volume.status, "AVAILABLE");
    let heads = server.heads();
    assert!(heads[1].starts_with(b"GET /server/v2/project-1/servers/server-1 "));
    assert!(
        heads[2].starts_with(b"PUT /server/v2/project-1/volumes/volume-1/servers/server-1/detach ")
    );
}

#[tokio::test]
async fn delete_requires_explicit_detachment_and_settles_on_not_found() {
    let (ctx, _) = RequestContext::new();
    let input = DeleteVolumeInput {
        volume_id: "volume-1".into(),
        no_wait: false,
    };
    let server = Server::start(vec![
        response(IN_USE),
        response(r#"{"data":{"uuid":"volume-1","status":"AVAILABLE"}}"#),
    ]);
    let volumes = client(&server, "hcm-3");
    let failure = volumes
        .delete_volume(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteRefusal>(),
        Some(&WriteRefusal::VolumeInUse)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let failure = volumes
        .delete_volume(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.requests(), 2);
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, ""),
        reply("404 Not Found", "{}"),
    ]);
    client(&server, "hcm-3")
        .delete_volume(&ctx, input)
        .await
        .unwrap();
    assert!(server.heads()[1].starts_with(b"DELETE /server/v2/project-1/volumes/volume-1 "));
}

#[tokio::test]
async fn create_checks_inventory_and_quotes_before_sending_once() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply("503 Service Unavailable", "{}"),
    ]);
    let error = client(&server, "han-1")
        .create_volume(&ctx, create())
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.partial().is_none());
    assert_eq!(
        error.recovery_action(),
        Some(crate::RecoveryAction::CheckExistingVolume)
    );
    assert_eq!(error.error().api_error().unwrap().status(), Some(503));
    assert_eq!(server.requests(), 3);
    let heads = server.heads();
    assert!(
        heads[0]
            .starts_with(b"GET /server/v2/project-1/volumes?name=toy-volume&page=1&size=10000 ")
    );
    assert!(heads[1].starts_with(b"POST /portal/v1/price "));
    assert!(heads[2].starts_with(b"POST /server/v2/project-1/volumes "));
    let bodies = server.bodies();
    let order: serde_json::Value = serde_json::from_slice(&bodies[2]).unwrap();
    assert!(
        order
            == serde_json::json!({"name":"toy-volume","size":1,"volumeTypeId":"type-1","zoneId":"zone-1","isEnableAutoRenew":false})
    );
}

#[tokio::test]
async fn ambiguous_create_id_never_polls_and_duplicate_name_never_quotes() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response(
        r#"{"listData":[{"uuid":"volume-1","name":"toy-volume"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
    )]);
    let failure = client(&server, "hcm-3")
        .create_volume(&ctx, create())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(
            ACCEPTED,
            r#"{"data":{"uuid":"volume-1","UUID":"volume-2"}}"#,
        ),
    ]);
    let failure = client(&server, "hcm-3")
        .create_volume(&ctx, create())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let api = failure.error().api_error().unwrap();
    assert_eq!((api.status(), api.code()), (Some(202), "InvalidResponse"));
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn resize_uses_same_type_in_quote_and_write_and_keeps_safe_partial() {
    let (ctx, _) = RequestContext::new();
    let reflected = AVAILABLE.replace("toy-volume", "synthetic%2dvolume%2dbearer");
    let refused = Server::start(vec![response(&reflected)]);
    let failure = client(&refused, "hcm-3")
        .resize_volume(&ctx, resize())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(refused.requests(), 1);
    let server = Server::start(vec![
        response(AVAILABLE),
        response(QUOTE),
        reply("503 Service Unavailable", "{}"),
    ]);
    let failure = client(&server, "hcm-3")
        .resize_volume(&ctx, resize())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.partial().unwrap().volume.name, "toy-volume");
    assert_eq!(server.requests(), 3);
    let heads = server.heads();
    assert!(heads[1].starts_with(b"POST /portal/v1/price "));
    assert!(heads[2].starts_with(b"PUT /server/v2/project-1/volumes/volume-1/resize "));
    let bodies = server.bodies();
    let quote: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    let write: serde_json::Value = serde_json::from_slice(&bodies[2]).unwrap();
    assert!(
        quote["resourceInfo"]
            == serde_json::json!({"newSize":2,"newVolumeTypeId":"type-1","volumeId":"volume-1"})
    );
    assert!(write == serde_json::json!({"newSize":2,"newVolumeTypeId":"type-1"}));
}

#[tokio::test]
async fn resize_does_not_confirm_truncated_acknowledgement() {
    let server = Server::start(vec![
        response(AVAILABLE),
        response(QUOTE),
        Response::Http {
            status: ACCEPTED.into(),
            headers: vec![("Content-Length".into(), "48".into())],
            body: b"{}".to_vec(),
        },
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "hcm-3")
        .resize_volume(&ctx, resize())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn resize_price_guard_blocks_unpriced_and_over_cap_quotes() {
    let (ctx, _) = RequestContext::new();
    for (price, class) in [
        ("0", ErrorClass::Unpriced),
        ("3201", ErrorClass::PriceAboveMax),
    ] {
        let server = Server::start(vec![
            response(AVAILABLE),
            response(&format!(r#"{{"optimumPrice":{price}}}"#)),
        ]);
        let failure = client(&server, "hcm-3")
            .resize_volume(&ctx, resize())
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), class);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn cancelled_confirmation_keeps_the_prewrite_volume_as_partial() {
    let (waiting, started) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, ""),
        Response::StallBeforeHead(waiting),
    ]);
    let (ctx, cancel) = RequestContext::new();
    let volume = client(&server, "hcm-3");
    let (result, ()) = tokio::join!(
        volume.attach_volume(
            &ctx,
            AttachVolumeInput {
                volume_id: "volume-1".into(),
                server_id: "server-1".into(),
                no_wait: false
            }
        ),
        async {
            started.await.unwrap();
            cancel.cancel();
        },
    );
    let failure = result.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert!(failure.error().has_class(ErrorClass::Cancelled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let output = failure.partial().unwrap();
    assert!(output.changed);
    assert_eq!(output.volume.status, "AVAILABLE");
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn failed_create_returns_the_confirmed_error_state() {
    let failed = AVAILABLE.replace("AVAILABLE", "ERROR");
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(ACCEPTED, r#"{"data":{"uuid":"volume-1"}}"#),
        response(&failed),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "hcm-3")
        .create_volume(&ctx, create())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Failed);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.partial().unwrap().volume.status, "ERROR");
    assert_eq!(server.requests(), 4);
}

fn changing_client(server: &Server, tokens: &'static [&'static str]) -> Client {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Provider(&'static [&'static str], AtomicUsize);
    impl crate::CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            Box::pin(async move {
                let index = self.1.fetch_add(1, Ordering::Relaxed);
                let value = self.0.get(index).copied().unwrap_or("toy-last-bearer");
                Ok(crate::Token::new(crate::Secret::new(value), None))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a crate::Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let config = crate::Config::new(
        crate::ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_credentials_provider(Arc::new(Provider(tokens, AtomicUsize::new(0))))
            .with_endpoint_overrides(crate::EndpointOverrides {
                vserver: Some(format!("{}/server/", server.origin)),
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

#[tokio::test]
async fn volume_write_attempt_rechecks_volume_and_detachment_proofs() {
    let ctx = RequestContext::new().0;
    let attached = r#"{"data":{"uuid":"volume-1","status":"IN-USE","serverId":"server-1","serverIdList":["server-1"]}}"#;
    for (action, reflects) in [
        ("delete", false),
        ("delete", true),
        ("attach", true),
        ("detach", true),
    ] {
        let note = if reflects {
            "toy-last-bearer"
        } else {
            "ordinary-note"
        };
        let mut volume: serde_json::Value = serde_json::from_str(if action == "detach" {
            attached
        } else {
            AVAILABLE
        })
        .unwrap();
        let mut server_detail =
            serde_json::json!({"data":{"status":"STOPPED","bootVolumeId":"boot-1"}});
        if action == "detach" {
            server_detail["data"]["note"] = note.into();
        } else {
            volume["data"]["note"] = note.into();
        }
        let mut replies = vec![response(&volume.to_string())];
        if action == "detach" {
            replies.push(response(&server_detail.to_string()));
        }
        replies.push(reply(ACCEPTED, ""));
        let server = Server::start(replies);
        let tokens: &'static [&'static str] = if action == "detach" {
            &["toy-read-bearer", "toy-server-bearer"]
        } else {
            &["toy-read-bearer"]
        };
        let client = changing_client(&server, tokens);
        let result = match action {
            "delete" => client
                .delete_volume(
                    &ctx,
                    DeleteVolumeInput {
                        volume_id: "volume-1".into(),
                        no_wait: true,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|error| error.write_dispatch()),
            "attach" => client
                .attach_volume(
                    &ctx,
                    AttachVolumeInput {
                        volume_id: "volume-1".into(),
                        server_id: "server-1".into(),
                        no_wait: true,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|error| error.write_dispatch()),
            _ => client
                .detach_volume(
                    &ctx,
                    DetachVolumeInput {
                        volume_id: "volume-1".into(),
                        server_id: "server-1".into(),
                        no_wait: true,
                        allow_running: false,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|error| error.write_dispatch()),
        };
        assert_eq!(
            result,
            if reflects {
                Err(WriteDispatch::NotSent)
            } else {
                Ok(())
            },
            "{action}"
        );
        assert_eq!(
            server.requests(),
            1 + usize::from(action == "detach") + usize::from(!reflects)
        );
    }
}

#[tokio::test]
async fn quote_credential_is_rechecked_before_ordering() {
    let server = Server::start(vec![response(EMPTY), response(QUOTE)]);
    let (ctx, _) = RequestContext::new();
    let failure = changing_client(&server, &["toy-inventory-bearer", "toy-quote-bearer"])
        .create_volume(
            &ctx,
            CreateVolumeInput {
                name: "toy%2dquote%2dbearer".into(),
                ..create()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn acquired_and_refreshed_tokens_are_screened_against_original_query() {
    let (ctx, _) = RequestContext::new();
    for (name, sends) in [
        ("toy%2dfirst%2dbearer", 0),
        ("toy%2drefresh%2dbearer", 1),
        ("toy%25252dvalue", 0),
    ] {
        let server = Server::start(vec![
            reply("401 Unauthorized", "{}"),
            reply("403 Forbidden", "{}"),
        ]);
        let failure = changing_client(&server, &["toy-first-bearer", "toy-refresh-bearer"])
            .create_volume(
                &ctx,
                CreateVolumeInput {
                    name: name.into(),
                    ..create()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(server.requests(), sends);
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    }
}

#[tokio::test]
async fn invalid_size_diagnostic_omits_a_bearer_acquired_during_volume_read() {
    let current = AVAILABLE.replace("\"size\":1", "\"size\":1000000000");
    let server = Server::start(vec![response(&current)]);
    let (ctx, _) = RequestContext::new();
    let bearer = "731940128";
    let failure = changing_client(&server, &["731940128"])
        .quote_resize_volume(
            &ctx,
            ResizeVolumeInput {
                size: bearer.parse().unwrap(),
                ..resize()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert!(
        failure
            .error()
            .detail::<String>()
            .is_some_and(|message| !message.contains(bearer))
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn create_requires_strict_type_id_and_accepts_underscore_encryption_keys() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![reply("403 Forbidden", "{}")]);
    let failure = client(&server, "hcm-3")
        .create_volume(
            &ctx,
            CreateVolumeInput {
                volume_type_id: "type_1".into(),
                ..create()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 0);
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    let server = Server::start(vec![response(QUOTE)]);
    let result = client(&server, "hcm-3")
        .quote_create_volume(
            &ctx,
            CreateVolumeInput {
                encryption_type_id: "toy_encryption".into(),
                ..create()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.optimum_price, 3200.0);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn unsettled_confirmation_retains_permission_denied_api_cause() {
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, ""),
        reply("403 Forbidden", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "hcm-3")
        .delete_volume(
            &ctx,
            DeleteVolumeInput {
                volume_id: "volume-1".into(),
                no_wait: false,
            },
        )
        .await
        .unwrap_err();
    let error = failure.error();
    assert_eq!(error.class(), ErrorClass::NotSettled);
    assert!(error.has_class(ErrorClass::PermissionDenied));
    let cause = error.api_error().unwrap();
    assert_eq!(cause.status(), Some(403));
    assert_eq!(cause.operation(), "volume.GetVolume");
    assert_eq!(server.requests(), 3);
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[tokio::test]
    async fn rounded_quote_is_rechecked_with_the_volume_order_bearer() {
        for (tokens, allowed) in [
            (&["toy-inventory", "toy-quote", "50.00"][..], false),
            (&["toy-inventory", "toy-quote", "50.01"][..], true),
        ] {
            let server = Server::start(vec![
                response(EMPTY),
                response(r#"{"optimumPrice":49.999}"#),
                reply(ACCEPTED, r#"{"data":{"uuid":"volume-1"}}"#),
            ]);
            let result = changing_client(&server, tokens)
                .create_volume(
                    &RequestContext::new().0,
                    CreateVolumeInput {
                        no_wait: true,
                        ..create()
                    },
                )
                .await;
            assert_eq!(
                result.map(|_| ()).map_err(|error| error.write_dispatch()),
                if allowed {
                    Ok(())
                } else {
                    Err(WriteDispatch::NotSent)
                }
            );
            assert_eq!(server.requests(), if allowed { 3 } else { 2 });
        }
    }

    #[tokio::test]
    async fn a_reflecting_volume_404_does_not_confirm_deletion() {
        for (body, allowed) in [
            ("{}", true),
            (r#"{"note":"synthetic-volume-bearer"}"#, false),
        ] {
            let server = Server::start(vec![
                response(AVAILABLE),
                reply(ACCEPTED, ""),
                reply("404 Not Found", body),
            ]);
            let result = client(&server, "hcm-3")
                .delete_volume(
                    &RequestContext::new().0,
                    DeleteVolumeInput {
                        volume_id: "volume-1".into(),
                        no_wait: false,
                    },
                )
                .await;
            assert_eq!(result.is_ok(), allowed);
            assert_eq!(server.requests(), 3);
        }
    }

    #[tokio::test]
    async fn volume_mutation_guards_reject_lone_folded_operand_names() {
        for field in ["serverId", "status"] {
            let variant = AVAILABLE.replace(
                &format!("\"{field}\""),
                &format!("\"{}\"", field.to_uppercase()),
            );
            let server = Server::start(vec![response(&variant), reply(ACCEPTED, "")]);
            let result = client(&server, "hcm-3")
                .delete_volume(
                    &RequestContext::new().0,
                    DeleteVolumeInput {
                        volume_id: "volume-1".into(),
                        no_wait: true,
                    },
                )
                .await;
            let Err(error) = result else {
                panic!("accepted a folded {field} operand");
            };
            assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
            assert_eq!(
                error.error().api_error().map(crate::ApiError::code),
                Some("InvalidResponse")
            );
            assert_eq!(server.requests(), 1);
        }
    }
    #[tokio::test]
    async fn attachment_receipt_rechecks_server_id_list_with_identifier_rules() {
        for (tokens, allowed) in [
            (&["toy-volume-read", "jkl"][..], false),
            (&["toy-volume-read", "mno"][..], true),
        ] {
            let detail = IN_USE
                .replace("server-1", "pre6A6B6Cpost")
                .replace("\"serverId\":\"pre6A6B6Cpost\"", "\"serverId\":null");
            let server = Server::start(vec![response(&detail), reply(ACCEPTED, "")]);
            let result = changing_client(&server, tokens)
                .attach_volume(
                    &RequestContext::new().0,
                    AttachVolumeInput {
                        volume_id: "volume-1".into(),
                        server_id: "server-1".into(),
                        no_wait: true,
                    },
                )
                .await;
            assert_eq!(
                result.map(|_| ()).map_err(|error| error.write_dispatch()),
                if allowed {
                    Ok(())
                } else {
                    Err(WriteDispatch::NotSent)
                }
            );
            assert_eq!(server.requests(), if allowed { 2 } else { 1 });
        }
    }
}

#[tokio::test]
async fn oversized_create_keeps_the_body_limit_cause_and_recovery_action() {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        Response::RawClose(
            format!(
                "HTTP/1.1 202 Accepted\r\nContent-Length: {}\r\n\r\n",
                (8 << 20) + 1
            )
            .into_bytes(),
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "han-1")
        .create_volume(&ctx, create())
        .await
        .unwrap_err();
    let error = failure.error();
    assert_eq!(error.api_error().unwrap().status(), Some(202));
    assert_eq!(
        error.api_error().unwrap().to_string(),
        "transport: response body exceeds limit"
    );
    assert_eq!(
        failure.recovery_action(),
        Some(crate::RecoveryAction::CheckExistingVolume)
    );
    assert_eq!(error.class(), ErrorClass::Api);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 3);
}

mod recovery;
