mod child_deletes;
mod create_writes;

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, Secret};

const CANARY: &str = "toy-secret";

fn response(status: &str, raw: impl AsRef<[u8]>) -> Response {
    let raw = raw.as_ref();
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.to_vec(),
    }
}

fn client(server: &Server, token: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new(token))
            .with_endpoint_overrides(EndpointOverrides {
                vlb: Some(format!("{}/regional/", server.origin)),
                portal: Some(format!("{}/billing/", server.origin)),
                ..EndpointOverrides::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

fn pool_members() -> ListPoolMembersInput {
    ListPoolMembersInput {
        load_balancer_id: "lb-1".into(),
        pool_id: "pool-1".into(),
    }
}

#[tokio::test]
async fn child_list_distinguishes_empty_missing_and_wrong_shape() {
    let server = Server::start(
        [
            r#"{"data":[]}"#,
            r#"{}"#,
            r#"{"data":null}"#,
            r#"{"data":{}}"#,
            "{",
            r#"{"data":[],"Data":[]}"#,
        ]
        .into_iter()
        .map(|raw| response("200 OK", raw))
        .collect(),
    );
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .list_pool_members(&ctx, pool_members())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    for detail in [
        None,
        None,
        Some(DecodeError::InvalidShape),
        Some(DecodeError::MalformedJson),
        Some(DecodeError::InvalidShape),
    ] {
        let error = client
            .list_pool_members(&ctx, pool_members())
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(api.status(), None);
        assert_eq!(api.operation(), "loadbalancer.ListPoolMembers");
        let message = if detail.is_some() {
            "loadbalancer.ListPoolMembers: request failed: response failed to decode"
        } else {
            "list response had no data"
        };
        assert_eq!(api.message(), message);
        assert_eq!(error.error().detail::<DecodeError>().copied(), detail);
    }
    assert!(server.heads().iter().all(|head| {
        head.starts_with(b"GET /regional/v2/proj-1/loadBalancers/lb-1/pools/pool-1/members ")
    }));
}

#[tokio::test]
async fn credentials_are_screened_in_inputs_and_projected_output() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"data":[{"uuid":"member_1","name":" toy%2Dsecret ","weight":-0}]}"#,
    )]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    for id in ["../lb", "lb_1", CANARY] {
        let input = GetLoadBalancerInput {
            load_balancer_id: id.into(),
        };
        let error = client.get_load_balancer(&ctx, input).await.unwrap_err();
        assert!(error.error().has_class(ErrorClass::InvalidInput));
    }
    let error = client
        .list_packages(
            &ctx,
            ListPackagesInput {
                zone_id: "toy%2Dsecret".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(server.requests(), 0);

    let list = client
        .list_pool_members(&ctx, pool_members())
        .await
        .unwrap();
    assert_eq!(list.items[0].uuid, "member_1");
    assert_eq!(list.items[0].name, "<redacted>");
    assert_eq!(list.items[0].weight, 0);
}

#[tokio::test]
async fn folded_envelope_variants_collide_without_exact_key() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"Page":1,"PAGE":2,"listData":[]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, CANARY)
        .list_load_balancers(&ctx, ListLoadBalancersInput::default())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn lone_case_variants_are_ignored_and_collisions_rejected() {
    let server = Server::start(vec![
        response("200 OK", r#"{"Page":7,"listData":[{"Name":"ignored"}]}"#),
        response("200 OK", r#"{"page":1,"Page":2,"listData":[]}"#),
        response(
            "200 OK",
            r#"{"page":1,"listData":[{"name":"ok","Name":"bad"}]}"#,
        ),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let result = client
        .list_load_balancers(&ctx, ListLoadBalancersInput::default())
        .await
        .unwrap();
    assert_eq!(result.page, 0);
    assert_eq!(result.items[0].name, "");
    for _ in 0..2 {
        assert!(
            client
                .list_load_balancers(&ctx, ListLoadBalancersInput::default())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn models_handle_go_strings_and_block_number_pseudo_objects() {
    let server = Server::start(vec![
        response(
            "200 OK",
            b"{\"data\":{\"name\":\"a\xff\\ud800\",\"zoneId\":\"zone_api\",\"zone\":{\"name\":4}}}",
        ),
        response(
            "200 OK",
            r#"{"data":{"minSize":{"$serde_json::private::Number":"1"}}}"#,
        ),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let input = GetLoadBalancerInput {
        load_balancer_id: "lb-1".into(),
    };
    let result = client.get_load_balancer(&ctx, input.clone()).await.unwrap();
    assert_eq!(result.load_balancer.name, "a\u{fffd}\u{fffd}");
    assert_eq!(result.load_balancer.zone_id, "zone_api");
    let error = client.get_load_balancer(&ctx, input).await.unwrap_err();
    assert_eq!(
        error.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
}

#[tokio::test]
async fn quote_posts_only_priced_fields_to_portal() {
    let quote = r#"{"optimumPrice":12500,"propertiesPrice":[]}"#;
    let portal = Server::start(vec![response("200 OK", quote), response("200 OK", quote)]);
    let client = client(&portal, CANARY);
    let (ctx, _) = RequestContext::new();
    let create = CreateLoadBalancerInput {
        name: "demo".into(),
        package_id: "package-1".into(),
        r#type: "Layer 4".into(),
        scheme: "Internal".into(),
        subnet_id: "subnet-1".into(),
        zone_id: "zone_1".into(),
        max_price: 1.0,
        no_wait: true,
    };
    let blocked = CreateLoadBalancerInput {
        zone_id: "toy%2Dsecret".into(),
        ..create.clone()
    };
    assert!(
        client
            .quote_create_load_balancer(&ctx, blocked)
            .await
            .unwrap_err()
            .error()
            .has_class(ErrorClass::InvalidInput)
    );
    assert_eq!(portal.requests(), 0);
    client
        .quote_create_load_balancer(&ctx, create)
        .await
        .unwrap();
    client
        .quote_resize_load_balancer(
            &ctx,
            ResizeLoadBalancerInput {
                load_balancer_id: "lb-1".into(),
                package_id: "package-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(portal.requests(), 2);
    assert!(
        portal
            .heads()
            .iter()
            .all(|head| head.starts_with(b"POST /billing/v1/price "))
    );
    let expected = [
        serde_json::json!({"resourceType":"load-balancer","action":"create","resourceInfo":{"packageId":"package-1","zoneId":"zone_1","isBuyMorePoc":false,"period":1,"isPoc":false}}),
        serde_json::json!({"resourceType":"load-balancer","action":"resize","resourceInfo":{"loadBalancerId":"lb-1","packageId":"package-1"}}),
    ];
    for (raw, expected) in portal.bodies().iter().zip(expected) {
        let body: serde_json::Value = serde_json::from_slice(raw).unwrap();
        assert!(body == expected, "unexpected quote request shape");
    }
}

#[tokio::test]
async fn load_balancer_delete_reports_preflight_absence_without_sending() {
    let server = Server::start(vec![response("404 Not Found", "{}")]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let error = client
        .delete_load_balancer(&ctx, delete_input(false))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotFound));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

fn delete_input(no_wait: bool) -> DeleteLoadBalancerInput {
    DeleteLoadBalancerInput {
        load_balancer_id: "lb-1".into(),
        no_wait,
    }
}

fn lb_status(status: &str) -> Response {
    response(
        "200 OK",
        format!(r#"{{"data":{{"uuid":"lb-1","progressStatus":"{status}"}}}}"#),
    )
}

#[tokio::test]
async fn load_balancer_delete_skips_an_in_progress_delete_and_obeys_no_wait() {
    for (status, methods) in [("DELETING", &[][..]), ("ERROR", &["DELETE"][..])] {
        let mut replies = vec![lb_status(status)];
        replies.extend(methods.iter().map(|_| response("202 Accepted", "")));
        let server = Server::start(replies);
        let client = client(&server, CANARY);
        let (ctx, _) = RequestContext::new();
        client
            .delete_load_balancer(&ctx, delete_input(true))
            .await
            .unwrap();
        assert_eq!(server.requests(), 1 + methods.len());
        for (head, method) in server
            .heads()
            .iter()
            .zip(["GET"].into_iter().chain(methods.iter().copied()))
        {
            assert!(head.starts_with(
                format!("{method} /regional/v2/proj-1/loadBalancers/lb-1 ").as_bytes()
            ));
        }
    }
}

#[tokio::test]
async fn load_balancer_delete_refuses_to_confirm_an_oversized_404() {
    let overflow = Response::Http {
        status: "404 Not Found".into(),
        headers: vec![("Content-Length".into(), ((8 << 20) + 1).to_string())],
        body: Vec::new(),
    };
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", ""),
        overflow,
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let error = client
        .delete_load_balancer(&ctx, delete_input(false))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotSettled));
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.partial(), Some(&DeleteLoadBalancerOutput {}));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn certificate_absence_requires_a_complete_inventory_after_a_failure() {
    for (body, absent) in [
        (
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            true,
        ),
        (
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
            false,
        ),
        (
            r#"{"listData":[{"uuid":"cert-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
            false,
        ),
        (
            r#"{"listData":[{"uuid":"toy%2Dsecret"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
            false,
        ),
        (
            r#"{"listData":null,"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            false,
        ),
    ] {
        let server = Server::start(vec![
            response(
                "400 Bad Request",
                r#"{"message":"certificate detail failed"}"#,
            ),
            response("200 OK", body),
        ]);
        let client = client(&server, CANARY);
        let (ctx, _) = RequestContext::new();
        let error = client
            .delete_certificate(
                &ctx,
                DeleteCertificateInput {
                    certificate_id: "cert-1".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.error().has_class(ErrorClass::NotFound), absent);
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
        assert!(
            server.heads()[1].starts_with(b"GET /regional/v2/proj-1/cas?name=&page=1&size=10000 ")
        );
    }
}

#[tokio::test]
async fn delete_preflight_refuses_ambiguous_target_or_missing_usage() {
    for (body, certificate) in [
        (
            r#"{"data":{"uuid":"toy-secret","progressStatus":"CREATED"}}"#,
            false,
        ),
        (r#"{"uuid":"cert-1"}"#, true),
        (r#"{"uuid":"cert-1","inUse":false,"InUse":false}"#, true),
    ] {
        let mut responses = vec![response("200 OK", body)];
        if certificate {
            responses.push(response(
                "200 OK",
                r#"{"listData":[{"uuid":"cert-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
            ));
        }
        let server = Server::start(responses);
        let client = client(&server, CANARY);
        let (ctx, _) = RequestContext::new();
        let dispatch = if certificate {
            client
                .delete_certificate(
                    &ctx,
                    DeleteCertificateInput {
                        certificate_id: "cert-1".into(),
                    },
                )
                .await
                .unwrap_err()
                .write_dispatch()
        } else {
            client
                .delete_load_balancer(&ctx, delete_input(false))
                .await
                .unwrap_err()
                .write_dispatch()
        };
        assert_eq!(dispatch, WriteDispatch::NotSent);
        assert_eq!(server.requests(), if certificate { 2 } else { 1 });
        assert!(server.heads().iter().all(|head| head.starts_with(b"GET ")));
    }
}

#[tokio::test]
async fn load_balancer_delete_retries_the_delete_before_polling() {
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("502 Bad Gateway", "{}"),
        response("202 Accepted", ""),
        response("404 Not Found", "{}"),
    ]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    client
        .delete_load_balancer(&ctx, delete_input(false))
        .await
        .unwrap();
    assert_eq!(server.requests(), 4);
    for (head, method) in server
        .heads()
        .iter()
        .zip(["GET", "DELETE", "DELETE", "GET"])
    {
        assert!(
            head.starts_with(
                format!("{method} /regional/v2/proj-1/loadBalancers/lb-1 ").as_bytes()
            )
        );
    }
}

#[tokio::test]
async fn certificate_absence_rejects_missing_or_null_pagination_totals() {
    for totals in [
        r#""#,
        r#", "totalPage":0"#,
        r#", "totalItem":0"#,
        r#", "totalPage":null, "totalItem":0"#,
        r#", "totalPage":0, "totalItem":null"#,
    ] {
        let inventory = format!(r#"{{"listData":[],"page":1,"pageSize":10000{totals}}}"#);
        let server = Server::start(vec![
            response("400 Bad Request", r#"{"message":"detail rejected"}"#),
            response("200 OK", inventory),
        ]);
        let client = client(&server, CANARY);
        let (ctx, _) = RequestContext::new();
        let error = client
            .delete_certificate(
                &ctx,
                DeleteCertificateInput {
                    certificate_id: "cert-1".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().status(), Some(400));
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn waiting_for_an_existing_delete_preserves_this_calls_dispatch_state() {
    let server = Server::start(vec![lb_status("DELETING"), response("403 Forbidden", "{}")]);
    let client = client(&server, CANARY);
    let (ctx, _) = RequestContext::new();
    let error = client
        .delete_load_balancer(&ctx, delete_input(false))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotSettled));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn complete_response_classification_precedes_public_redaction() {
    let lb = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", ""),
        response("404 Not Found", "{}"),
    ]);
    let cert = Server::start(vec![
        response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#),
        response(
            "409 Conflict",
            r#"{"message":"certificate is in use by toy%2Dsecret"}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let deleted = client(&lb, "404")
        .delete_load_balancer(&ctx, delete_input(false))
        .await;
    let error = client(&cert, CANARY)
        .delete_certificate(
            &ctx,
            DeleteCertificateInput {
                certificate_id: "cert-1".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<WriteError>(),
        Some(&WriteError::ResourceInUse)
    );
    assert!(
        !error
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message()
            .contains("toy%2Dsecret")
    );
    assert!(deleted.is_ok());
    assert_eq!(lb.requests(), 3);
    assert_eq!(cert.requests(), 2);
}

#[tokio::test]
async fn malformed_certificate_detail_reconciles_without_dispatching_delete() {
    let server = Server::start(vec![
        response("200 OK", "{"),
        response(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, CANARY)
        .delete_certificate(
            &ctx,
            DeleteCertificateInput {
                certificate_id: "cert-1".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotFound));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
    assert!(server.heads().iter().all(|head| head.starts_with(b"GET ")));
}

#[tokio::test]
async fn delete_settlement_distinguishes_cancellation_poll_failure_and_bound() {
    let (arrived, poll_started) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", ""),
        Response::StallBeforeHead(arrived),
    ]);
    let (ctx, cancel) = RequestContext::new();
    let service = client(&server, CANARY);
    let (result, ()) = tokio::join!(
        service.delete_load_balancer(&ctx, delete_input(false)),
        async {
            poll_started.await.unwrap();
            cancel.cancel();
        }
    );
    let cancelled = result.unwrap_err();

    let server = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", ""),
        response("403 Forbidden", r#"{"message":"poll denied"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let refused = client(&server, CANARY)
        .delete_load_balancer(&ctx, delete_input(false))
        .await
        .unwrap_err();

    let (arrived, poll_started) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        lb_status("CREATED"),
        response("202 Accepted", ""),
        Response::StallBeforeHead(arrived),
    ]);
    let service = client(&server, CANARY);
    let (result, ()) = tokio::join!(
        service.delete_load_balancer(&ctx, delete_input(false)),
        async {
            poll_started.await.unwrap();
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(15 * 60)).await;
        }
    );
    let expired = result.unwrap_err();

    for (error, message) in [
        (&cancelled, "cancelled"),
        (&refused, "poll denied"),
        (&expired, "did not reach 404 within 15m0s"),
    ] {
        assert!(error.error().has_class(ErrorClass::NotSettled));
        assert!(
            error
                .error()
                .detail::<crate::RecoveryAdvice>()
                .unwrap()
                .message()
                .contains(message),
            "missing settlement reason {message}"
        );
    }
    assert!(cancelled.error().has_class(ErrorClass::Cancelled));
    assert_eq!(refused.error().api_error().unwrap().status(), Some(403));
    assert!(expired.error().has_class(ErrorClass::DeadlineExceeded));
}
