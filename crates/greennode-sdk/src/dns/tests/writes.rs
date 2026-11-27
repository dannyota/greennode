use super::*;
use serde_json::{Value, json};

fn body(server: &Server, index: usize) -> Value {
    serde_json::from_slice(&server.bodies()[index]).unwrap()
}

fn client_with_confirmation_bearer(server: &Server, bearer: &'static str) -> Client {
    client_with_rotating_bearer(server, bearer, 2)
}

fn client_with_rotating_bearer(server: &Server, bearer: &'static str, after: usize) -> Client {
    use crate::{CredentialsProvider, Token};
    use std::future::{Future, ready};
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Provider {
        calls: AtomicUsize,
        confirmation_bearer: &'static str,
        after: usize,
    }
    impl CredentialsProvider for Provider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            let value = if self.calls.fetch_add(1, Ordering::Relaxed) < self.after {
                BEARER
            } else {
                self.confirmation_bearer
            };
            Box::pin(ready(Ok(Token::new(Secret::new(value), None))))
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(ready(Ok(())))
        }
    }
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_credentials_provider(Arc::new(Provider {
                calls: AtomicUsize::new(0),
                confirmation_bearer: bearer,
                after,
            }))
            .with_endpoint_overrides(EndpointOverrides {
                dns: Some(format!("{}/vdns-api/", server.origin)),
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
async fn readiness_status_is_rechecked_with_the_write_bearer() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":"","assocVpcIds":[]}}"#,
        ),
        response("204 No Content", ""),
    ]);
    let failure = client_with_rotating_bearer(&server, "ACTIVE", 1)
        .update_hosted_zone(
            &RequestContext::new().0,
            UpdateHostedZoneInput {
                description: Some("new".into()),
                no_wait: true,
                ..UpdateHostedZoneInput::new("zone-1")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn replacement_rechecks_copied_association_ids_against_the_write_bearer() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":"","assocVpcIds":["pre6A6B6Cpost"]}}"#,
        ),
        response("204 No Content", ""),
    ]);
    let failure = client_with_rotating_bearer(&server, "jkl", 1)
        .update_hosted_zone(
            &RequestContext::new().0,
            UpdateHostedZoneInput {
                description: Some("new".into()),
                no_wait: true,
                ..UpdateHostedZoneInput::new("zone-1")
            },
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn readiness_refuses_a_reflected_active_state() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":"","assocVpcIds":[]}}"#,
        ),
        response("204 No Content", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "ACTIVE")
        .update_hosted_zone(
            &ctx,
            UpdateHostedZoneInput {
                description: Some("new".into()),
                no_wait: true,
                ..UpdateHostedZoneInput::new("zone-1")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn replacement_requires_complete_matching_evidence() {
    for snapshot in [
        r#"{"data":{"hostedZoneId":"wrong-id","status":"ACTIVE","description":"","assocVpcIds":[]}}"#,
        r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":""}}"#,
        r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":null,"assocVpcIds":[]}}"#,
        r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":"","assocVpcIds":null}}"#,
        r#"{"data":{"hostedZoneId":"zone-1","status":"ACTIVE","description":"","assocVpcIds":["synthetic-sdk-dns-bearer"]}}"#,
    ] {
        let server = Server::start(vec![response("200 OK", snapshot)]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, BEARER)
            .update_hosted_zone(
                &ctx,
                UpdateHostedZoneInput {
                    hosted_zone_id: "zone-1".into(),
                    description: Some("new".into()),
                    no_wait: true,
                    ..UpdateHostedZoneInput::new("zone-1")
                },
            )
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert!(failure.partial().is_none());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn replacement_sends_explicit_empty_associations_and_retains_safe_partial() {
    const DESCRIPTION: &str = "zone-migration-pending";
    let server = Server::start(vec![
        response(
            "200 OK",
            &ZONE.replace(r#""status""#, r#""description":"before","status""#),
        ),
        response("204 No Content", ""),
        response("403 Forbidden", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client_with_confirmation_bearer(&server, DESCRIPTION)
        .update_hosted_zone(
            &ctx,
            UpdateHostedZoneInput {
                hosted_zone_id: "zone-1".into(),
                description: Some(DESCRIPTION.into()),
                vpc_ids: Some(vec![]),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let zone = &failure.partial().unwrap().hosted_zone;
    assert_eq!(zone.id, "zone-1");
    assert!(zone.associated_vpc_ids.is_empty());
    assert!(zone.description == "<redacted>");
    assert_eq!(body(&server, 1)["assocVpcIds"], json!([]));
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn post_write_reads_cannot_replace_a_partial_with_the_wrong_record() {
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("200 OK", &RECORD.replace("zone-1", "another-zone")),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client_with_confirmation_bearer(&server, "610237")
        .update_record(
            &ctx,
            UpdateRecordInput {
                hosted_zone_id: "zone-1".into(),
                record_id: "record-1".into(),
                ttl: Some(610237),
                no_wait: true,
                ..UpdateRecordInput::new("zone-1", "record-1")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let record = &failure.partial().unwrap().record;
    assert_eq!(record.id, "record-1");
    assert!(record.ttl == Field::Redacted);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn record_confirmation_rechecks_the_earlier_zone_with_the_final_bearer() {
    let late = "toy-zone-description";
    let observed_zone = ZONE.replace(
        r#""status""#,
        &format!(r#""description":"{late}","status""#),
    );
    let created = include_str!("../../../tests/fixtures/dns/create_record.json");
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("200 OK", created),
        response("200 OK", &observed_zone),
        response("200 OK", RECORD),
    ]);
    let input = CreateRecordInput::new(
        "zone-1",
        "A",
        vec![RecordValue {
            value: "192.0.2.1".into(),
            ..Default::default()
        }],
    );
    let failure = client_with_rotating_bearer(&server, late, 3)
        .create_record(&RequestContext::new().0, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn record_create_uses_defaults_and_retains_terminal_error_result() {
    let created = include_str!("../../../tests/fixtures/dns/create_record.json");
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("200 OK", created),
        response("200 OK", ZONE),
        response("200 OK", &RECORD.replace("ACTIVE", "ERROR")),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .create_record(
            &ctx,
            CreateRecordInput {
                hosted_zone_id: "zone-1".into(),
                record_type: "A".into(),
                values: vec![RecordValue {
                    value: "192.0.2.1".into(),
                    ..Default::default()
                }],
                ..CreateRecordInput::new("zone-1", "A", vec![])
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Failed);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.partial().unwrap().record.status, "ERROR");
    assert_eq!(
        body(&server, 1),
        json!({
            "subDomain":"", "ttl":300, "type":"A", "routingPolicy":"simple-routing", "value":[{"value":"192.0.2.1"}]
        })
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn deletes_wait_for_not_found_without_resending() {
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("404 Not Found", ""),
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("404 Not Found", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    client
        .delete_record(
            &ctx,
            DeleteRecordInput {
                hosted_zone_id: "zone-1".into(),
                record_id: "record-1".into(),
                no_wait: false,
            },
        )
        .await
        .unwrap();
    client
        .delete_hosted_zone(
            &ctx,
            DeleteHostedZoneInput {
                hosted_zone_id: "zone-1".into(),
                no_wait: false,
            },
        )
        .await
        .unwrap();
    let heads = server.heads();
    for (head, path) in heads.iter().zip([
        "GET /vdns-api/v1/dns/hosted-zone/zone-1 ",
        "DELETE /vdns-api/v1/dns/hosted-zone/zone-1/record/record-1 ",
        "GET /vdns-api/v1/dns/hosted-zone/zone-1/record/record-1 ",
        "GET /vdns-api/v1/dns/hosted-zone/zone-1 ",
        "DELETE /vdns-api/v1/dns/hosted-zone/zone-1 ",
        "GET /vdns-api/v1/dns/hosted-zone/zone-1 ",
    ]) {
        assert!(head.starts_with(path.as_bytes()));
    }
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn create_is_sent_once_and_reports_a_missing_id() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":{"status":"CREATING"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .create_hosted_zone(
            &ctx,
            CreateHostedZoneInput::new("toy.example", vec!["vpc-1".into()]),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_none());
    assert_eq!(failure.error().api_error().unwrap().status(), Some(200));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn cloned_clients_serialize_until_settlement_and_cancel_waiters_before_dispatch() {
    let created = include_str!("../../../tests/fixtures/dns/create_hosted_zone.json");
    let (started, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response("200 OK", created),
        Response::StallBeforeHead(started),
        response("200 OK", created),
    ]);
    let client = client(&server, BEARER);
    let clone = client.clone();
    let (ctx, cancel) = RequestContext::new();
    let (waiting_ctx, cancel_waiter) = RequestContext::new();
    let first = client.create_hosted_zone(
        &ctx,
        CreateHostedZoneInput::new("first.test", vec!["vpc-1".into()]),
    );
    tokio::pin!(first);
    tokio::select! {
        result = &mut first => panic!("write ended before settlement: {}", result.is_ok()),
        ready = received => assert!(ready.is_ok()),
    }
    let second = clone.create_hosted_zone(
        &waiting_ctx,
        CreateHostedZoneInput {
            no_wait: true,
            ..CreateHostedZoneInput::new("second.test", vec!["vpc-1".into()])
        },
    );
    tokio::pin!(second);
    tokio::select! {
        biased;
        result = &mut second => panic!("waiting writer finished early: {}", result.is_ok()),
        () = tokio::task::yield_now() => {},
    }
    cancel_waiter.cancel();
    let failure = second.await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_class(ErrorClass::Cancelled));
    assert_eq!(server.requests(), 2);

    cancel.cancel();
    let failure = first.await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(failure.partial().unwrap().hosted_zone.id, "zone-1");
    let (ctx, _) = RequestContext::new();
    clone
        .create_hosted_zone(
            &ctx,
            CreateHostedZoneInput {
                no_wait: true,
                ..CreateHostedZoneInput::new("third.test", vec!["vpc-1".into()])
            },
        )
        .await
        .unwrap();
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn preflight_deadlines_cancel_stalled_reads_and_keep_dispatch_not_sent() {
    use std::time::Duration;

    for caller_timeout in [None, Some(Duration::from_secs(20))] {
        let (started, received) = tokio::sync::oneshot::channel();
        let server = Server::start(vec![Response::StallBeforeHead(started)]);
        let client = client(&server, BEARER);
        let (ctx, _) = RequestContext::new();
        let ctx = match caller_timeout {
            Some(timeout) => ctx.child_with_deadline(tokio::time::Instant::now() + timeout),
            None => ctx,
        };
        let delete = client.delete_hosted_zone(&ctx, DeleteHostedZoneInput::new("zone-1"));
        tokio::pin!(delete);
        tokio::select! {
            result = &mut delete => panic!("preflight ended before the read began: {}", result.is_ok()),
            ready = received => assert!(ready.is_ok()),
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(60)).await;
        let failure = delete.await.unwrap_err();
        tokio::time::resume();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 1);
        if caller_timeout.is_some() {
            assert_eq!(failure.error().class(), ErrorClass::DeadlineExceeded);
        } else {
            assert_eq!(
                failure.error().detail::<ErrorDetail>(),
                Some(&ErrorDetail::ZoneBusy)
            );
        }
    }
}

#[tokio::test]
async fn write_does_not_replay_retryable_responses_or_follow_redirects() {
    for reply in [
        response("429 Too Many Requests", ""),
        response("503 Service Unavailable", ""),
        Response::Raw(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /another-zone\r\nContent-Length: 0\r\n\r\n".to_vec()),
    ] {
        let server = Server::start(vec![reply, response("200 OK", ZONE)]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, BEARER).create_hosted_zone(&ctx, CreateHostedZoneInput::new("example.test", vec!["vpc-1".into()])).await.unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn strict_preflight_treats_an_empty_success_as_malformed_json() {
    let server = Server::start(vec![response("200 OK", "")]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .delete_hosted_zone(&ctx, DeleteHostedZoneInput::new("zone-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::MalformedJson)
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn a_delete_probe_preserves_404_when_the_error_body_exceeds_its_cap() {
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("404 Not Found", &"x".repeat((256 << 10) + 1)),
    ]);
    let (ctx, _) = RequestContext::new();
    client(&server, BEARER)
        .delete_hosted_zone(&ctx, DeleteHostedZoneInput::new("zone-1"))
        .await
        .unwrap();
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn settlement_diagnostics_omit_unsafe_ids() {
    use crate::{CredentialsProvider, Token};
    use std::future::{Future, ready};
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct FailsAfterCreation(AtomicBool);

    impl CredentialsProvider for FailsAfterCreation {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            let result = if self.0.swap(true, Ordering::Relaxed) {
                Err(Error::provider_failed())
            } else {
                Ok(Token::new(Secret::new(BEARER), None))
            };
            Box::pin(ready(result))
        }

        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(ready(Ok(())))
        }
    }

    let encoded = "%73ynthetic-sdk-dns-bearer";
    let (ctx, _) = RequestContext::new();
    let mut screened = Vec::new();
    for (id, fails_signin) in [
        (encoded, false),
        (BEARER, false),
        ("untracked-zone-id", true),
    ] {
        let created = format!(r#"{{"data":{{"hostedZoneId":"{id}"}}}}"#);
        let server = Server::start(vec![response("200 OK", &created)]);
        let client = if fails_signin {
            let config = Config::new(
                ConfigOptions::default()
                    .with_region("hcm-3".into())
                    .with_credentials_provider(Arc::new(FailsAfterCreation(AtomicBool::new(false))))
                    .with_endpoint_overrides(EndpointOverrides {
                        dns: Some(server.origin.clone()),
                        ..Default::default()
                    }),
            )
            .unwrap();
            config
                .session()
                .set_test_product_root(server.root_der.clone());
            Client::new(&config)
        } else {
            client(&server, BEARER)
        };
        let failure = client
            .create_hosted_zone(
                &ctx,
                CreateHostedZoneInput::new("example.test", vec!["vpc-1".into()]),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotSettled);
        let message = failure
            .error()
            .detail::<String>()
            .map(String::as_str)
            .or_else(|| failure.error().api_error().map(ApiError::message))
            .unwrap();
        assert!(message.contains("hosted zone"));
        screened.push(!message.contains(id));
        if fails_signin {
            assert!(failure.partial().is_none());
        }
        assert_eq!(server.requests(), 1);
    }
    assert_eq!(screened, [true, true, true]);
}

#[tokio::test]
async fn a_settlement_error_keeps_the_actual_api_failure() {
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("404 Not Found", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .update_record(
            &ctx,
            UpdateRecordInput {
                ttl: Some(40),
                no_wait: true,
                ..UpdateRecordInput::new("zone-1", "record-1")
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotSettled);
    assert_eq!(failure.error().api_error().unwrap().status(), Some(404));
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().is_some());
}

#[tokio::test]
async fn partial_dns_resources_withhold_malformed_and_credential_bearing_ids() {
    let encoded = "%73ynthetic-sdk-dns-bearer";
    let zone_response = json!({"data": {
        "hostedZoneId": encoded,
        "assocVpcIds": ["vpc-1", BEARER, "malformed/id"],
        "assocVpcMapRegion": [{"vpcId":encoded,"region":"hcm-3"}]
    }})
    .to_string();
    let record_response = json!({"data": {
        "recordId": "record/bad",
        "hostedZoneId": encoded
    }})
    .to_string();
    let server = Server::start(vec![
        response("200 OK", &zone_response),
        response("200 OK", ZONE),
        response("200 OK", &record_response),
        response("200 OK", ZONE),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    let zone = client
        .create_hosted_zone(
            &ctx,
            CreateHostedZoneInput::new("example.test", vec!["vpc-1".into()]),
        )
        .await
        .unwrap_err();
    let record = client
        .create_record(&ctx, CreateRecordInput::new("zone-1", "A", vec![]))
        .await
        .unwrap_err();
    assert_eq!(zone.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(record.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let zone = serde_json::to_value(zone.partial().unwrap()).unwrap();
    let record = serde_json::to_value(record.partial().unwrap()).unwrap();
    let screened = [
        zone["HostedZone"]["ID"] == "<redacted>",
        zone["HostedZone"]["AssociatedVPCIDs"] == json!(["vpc-1", "<redacted>", "<redacted>"]),
        zone["HostedZone"]["AssocVPCMapRegion"][0]["VPCID"] == "<redacted>",
        record["Record"]["ID"] == "<redacted>",
        record["Record"]["HostedZoneID"] == "<redacted>",
    ];
    assert_eq!(screened, [true; 5]);
    assert_eq!(server.requests(), 4);
}
