use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 4","progressStatus":"CREATED"}}"#;
const POOL: &str = r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED","members":[{"address":"192.0.2.10","protocolPort":80}]}}"#;
const MEMBER: &str =
    r#"{"data":[{"uuid":"member-1","address":"192.0.2.10","protocolPort":80,"weight":1}]}"#;
const MOVED_POOL: &str = r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED","members":[{"address":"192.0.2.12","protocolPort":80}]}}"#;
const MOVED_MEMBER: &str =
    r#"{"data":[{"uuid":"member-2","address":"192.0.2.12","protocolPort":80,"weight":1}]}"#;

fn add() -> AddPoolMemberInput {
    AddPoolMemberInput {
        load_balancer_id: "lb-1".into(),
        pool_id: "pool-1".into(),
        address: "192.0.2.11".into(),
        port: 80,
        no_wait: true,
        ..Default::default()
    }
}

fn preflight(pool: &str, members: &str) -> [Response; 3] {
    [
        response("200 OK", PARENT),
        response("200 OK", pool),
        response("200 OK", members),
    ]
}

#[tokio::test]
async fn member_busy_resend_rebuilds_from_both_fresh_reads() {
    let replies = preflight(POOL, MEMBER)
        .into_iter()
        .chain([response(
            "409 Conflict",
            r#"{"message":"pool is updating"}"#,
        )])
        .chain(preflight(MOVED_POOL, MOVED_MEMBER))
        .chain([response("202 Accepted", "")]);
    let server = Server::start(replies.collect());
    let (ctx, _) = RequestContext::new();
    let output = client(&server, CANARY)
        .add_pool_member(&ctx, add())
        .await
        .unwrap();
    assert!(output.changed);
    let mut members = output.pool.members.iter();
    assert_eq!(members.next().unwrap().address, "192.0.2.12");
    assert_eq!(members.next().unwrap().address, "192.0.2.11");
    assert!(members.next().is_none());
    assert_eq!(server.requests(), 8);
    let bodies = server.bodies();
    let replaces: Vec<serde_json::Value> = [3, 7]
        .into_iter()
        .map(|i| serde_json::from_slice(&bodies[i]).unwrap())
        .collect();
    assert_eq!(replaces[0]["members"][0]["ipAddress"], "192.0.2.10");
    assert_eq!(replaces[1]["members"][0]["ipAddress"], "192.0.2.12");
}

#[tokio::test]
async fn member_once_policy_disallows_transport_resends() {
    for (status, unsettled) in [("502 Bad Gateway", true), ("429 Too Many Requests", false)] {
        let replies = preflight(POOL, MEMBER)
            .into_iter()
            .chain([response(status, "{}"), response("202 Accepted", "")]);
        let server = Server::start(replies.collect());
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .add_pool_member(&ctx, add())
            .await
            .unwrap_err();
        assert_eq!(failure.error().has_class(ErrorClass::NotSettled), unsettled);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().unwrap().changed);
        assert_eq!(server.requests(), 4);
    }
}

#[tokio::test]
async fn member_reads_refuse_disagreement_duplicates_and_aliases() {
    let duplicate = r#"{"data":[{"address":"192.0.2.10","protocolPort":80},{"address":"192.0.2.10","protocolPort":80}]}"#;
    let alias = r#"{"data":[{"address":"192.0.2.10","protocolPort":80,"Weight":1}]}"#;
    for (members, busy) in [(MOVED_MEMBER, true), (duplicate, true), (alias, false)] {
        let server = Server::start(preflight(POOL, members).into_iter().collect());
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, CANARY)
            .add_pool_member(&ctx, add())
            .await
            .unwrap_err();
        assert_eq!(
            failure.error().detail::<WriteError>(),
            busy.then_some(&WriteError::ResourceBusy)
        );
        assert_eq!(
            failure.error().detail::<DecodeError>(),
            (!busy).then_some(&DecodeError::InvalidShape)
        );
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 3);
    }
    let pool = r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED","Members":[{"address":"192.0.2.10","protocolPort":80}]}}"#;
    let server = Server::start(vec![response("200 OK", PARENT), response("200 OK", pool)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .add_pool_member(&ctx, add())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn identical_add_is_a_noop_and_absent_update_or_remove_is_not_found() {
    let server = Server::start(preflight(POOL, MEMBER).into_iter().collect());
    let (ctx, _) = RequestContext::new();
    let mut input = add();
    input.address = "192.0.2.10".into();
    let output = client(&server, CANARY)
        .add_pool_member(&ctx, input)
        .await
        .unwrap();
    assert!(!output.changed);
    assert_eq!(server.requests(), 3);
    let server = Server::start(
        preflight(POOL, MEMBER)
            .into_iter()
            .chain(preflight(POOL, MEMBER))
            .collect(),
    );
    let service = client(&server, CANARY);
    let absent_update = service
        .update_pool_member(
            &ctx,
            UpdatePoolMemberInput {
                load_balancer_id: "lb-1".into(),
                pool_id: "pool-1".into(),
                address: "192.0.2.11".into(),
                port: 80,
                weight: Some(3),
                no_wait: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let absent_remove = service
        .remove_pool_member(
            &ctx,
            RemovePoolMemberInput {
                load_balancer_id: "lb-1".into(),
                pool_id: "pool-1".into(),
                address: "192.0.2.11".into(),
                port: 80,
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert!(absent_update.error().has_class(ErrorClass::NotFound));
    assert!(absent_remove.error().has_class(ErrorClass::NotFound));
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn member_confirmation_reads_the_list_after_settlement_and_keeps_a_partial_on_mismatch() {
    let replies = preflight(POOL, MEMBER).into_iter().chain([
        response("202 Accepted", ""),
        response("200 OK", POOL),
        response("200 OK", PARENT),
        response("200 OK", MEMBER),
    ]);
    let server = Server::start(replies.collect());
    let (ctx, _) = RequestContext::new();
    let mut input = add();
    input.no_wait = false;
    let failure = settlement_client(&server)
        .add_pool_member(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().unwrap().changed);
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn incomplete_busy_response_never_authorizes_another_replacement() {
    let oversized = Response::Http {
        status: "409 Conflict".into(),
        headers: vec![("Content-Length".into(), ((8 << 20) + 1).to_string())],
        body: br#"{"message":"pool is updating"}"#.to_vec(),
    };
    let replies = preflight(POOL, MEMBER).into_iter().chain([oversized]);
    let server = Server::start(replies.collect());
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .add_pool_member(&ctx, add())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(409));
    assert_eq!(api.code(), "Conflict");
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.partial().unwrap().changed);
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn confirmed_member_comparison_is_independent_of_list_order() {
    let confirmed = r#"{"data":[{"address":"192.0.2.11","protocolPort":80,"weight":1},{"address":"192.0.2.10","protocolPort":80,"weight":1}]}"#;
    let replies = preflight(POOL, MEMBER).into_iter().chain([
        response("202 Accepted", ""),
        response("200 OK", POOL),
        response("200 OK", PARENT),
        response("200 OK", confirmed),
    ]);
    let server = Server::start(replies.collect());
    let (ctx, _) = RequestContext::new();
    let mut input = add();
    input.no_wait = false;
    let output = settlement_client(&server)
        .add_pool_member(&ctx, input)
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn member_write_reports_the_redirect_status_and_message() {
    let replies = preflight(POOL, MEMBER)
        .into_iter()
        .chain([super::create_writes::redirect()]);
    let server = Server::start(replies.collect());
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .add_pool_member(&ctx, add())
        .await
        .unwrap_err();
    super::create_writes::assert_reported_redirect(&failure);
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn member_required_fields_all_precede_path_and_address_checks() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, CANARY)
        .add_pool_member(
            &ctx,
            AddPoolMemberInput {
                address: "x".into(),
                port: 0,
                ..add()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        "vngcloud: invalid input: loadbalancer.AddPoolMember requires Port"
    );
    let failure = client(&server, CANARY)
        .add_pool_member(
            &ctx,
            AddPoolMemberInput {
                pool_id: "bad id".into(),
                port: 0,
                ..add()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        "vngcloud: invalid input: loadbalancer.AddPoolMember requires Port"
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn member_pre_write_timeout_names_load_balancer_and_pool() {
    let (arrived, started) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("200 OK", POOL),
        Response::StallBeforeHead(arrived),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, CANARY);
    let (result, ()) = tokio::join!(client.add_pool_member(&ctx, add()), async {
        started.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(600)).await;
    });
    let failure = result.unwrap_err();
    assert_eq!(
        failure
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message(),
        "loadbalancer: resource busy: loadbalancer.AddPoolMember: load balancer lb-1 or pool pool-1 is not ready within 10m0s; nothing sent"
    );
}

#[tokio::test]
async fn member_send_step_refuses_an_expired_pre_write_bound_with_the_pool_only_text() {
    let clock = std::sync::Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let server = Server::start(vec![]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new(CANARY))
            .with_clock(clock.clone())
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
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let ids = [("LoadBalancerID", "lb-1"), ("PoolID", "pool-1")];
    let mut write = crate::loadbalancer::child_writes::ChildWriter::new(
        &client,
        &ctx,
        "loadbalancer.AddPoolMember",
        "lb-1",
        &ids,
    )
    .unwrap();
    let pre = ctx.child_with_deadline(
        crate::Clock::monotonic_now(clock.as_ref()) + crate::loadbalancer::child_writes::BOUND,
    );
    clock.advance(crate::loadbalancer::child_writes::BOUND);
    let route = crate::loadbalancer::child_writes::ChildTarget {
        read: &crate::loadbalancer::member_state::MEMBERS,
        ..crate::loadbalancer::pool_writes::pool_target("pool-1", &ids)
    };
    let error = member_writes::send_replacement(&ctx, &pre, &mut write, &route, "{}".into())
        .await
        .unwrap_err();
    assert_eq!(
        error.detail::<crate::RecoveryAdvice>().unwrap().message(),
        "loadbalancer: resource busy: loadbalancer.AddPoolMember: pool is not ready; nothing sent"
    );
    assert_eq!(
        error.detail::<WriteError>(),
        Some(&WriteError::ResourceBusy)
    );
    assert_eq!(server.requests(), 0);
}
