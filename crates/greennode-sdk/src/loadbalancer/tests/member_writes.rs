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
    let failure = client(&server, CANARY)
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
    assert!(failure.error().is_body_too_large());
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
    let output = client(&server, CANARY)
        .add_pool_member(&ctx, input)
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(server.requests(), 7);
}
