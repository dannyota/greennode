use super::*;

#[tokio::test]
async fn paid_create_rejects_an_acknowledgement_without_clean_identifier_evidence() {
    for (bearer, raw) in [
        (
            "toy-ack-bearer",
            r#"{"data":{"uuid":"volume-1"},"ignored":"toy-ack-bearer"}"#,
        ),
        ("jkl", r#"{"data":{"uuid":"pre6A6B6Cpost"}}"#),
    ] {
        let server = Server::start(vec![
            response(EMPTY),
            response(QUOTE),
            reply("202 Accepted", raw),
        ]);
        let options = crate::ConfigOptions::default().with_static_token(crate::Secret::new(bearer));
        let mut input = create();
        input.no_wait = true;
        let failure = volume_with_options(&server, options, true)
            .create_volume(&RequestContext::new().0, input)
            .await
            .unwrap_err();
        assert_eq!(server.requests(), 3);
        assert!(failure.partial().is_none());
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(
            failure.recovery_action(),
            Some(crate::RecoveryAction::CheckExistingVolume)
        );
        assert_eq!(
            failure.error().api_error().unwrap().code(),
            "InvalidResponse"
        );
    }
}

#[tokio::test]
async fn cold_write_project_discovery_refuses_a_lone_alias() {
    let server = Server::start(vec![
        response(r#"{"projects":[{"ProjectID":"project-1","region":"han-1"}]}"#),
        response(EMPTY),
        response(QUOTE),
        reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
    ]);
    let mut input = create();
    input.no_wait = true;
    let failure = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-bearer")),
        false,
    )
    .create_volume(&RequestContext::new().0, input)
    .await
    .unwrap_err();
    assert_eq!(server.requests(), 1);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
}

#[tokio::test]
async fn incomplete_inventory_rechecks_an_earlier_page_after_token_rotation() {
    let early = r#"{"listData":[{"uuid":"volume-2","name":"prior-page-secret"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#;
    let server = Server::start(vec![response(early), response("{}")]);
    let options = crate::ConfigOptions::default()
        .with_credentials_provider(provider(["toy-first", "prior-page-secret"]));
    let failure = volume_with_options(&server, options, true)
        .create_volume(&RequestContext::new().0, create())
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 2);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
}

#[tokio::test]
async fn empty_inventory_is_incomplete_input_and_an_alias_is_a_guard_refusal() {
    for (body, incomplete) in [
        ("{}", true),
        (
            r#"{"ListData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            false,
        ),
    ] {
        let server = Server::start(vec![response(body)]);
        let options =
            crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-bearer"));
        let failure = volume_with_options(&server, options, true)
            .create_volume(&RequestContext::new().0, create())
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(
            failure.error().has_identity(ErrorClass::InvalidInput),
            incomplete
        );
        if incomplete {
            assert!(
                failure
                    .error()
                    .detail::<String>()
                    .is_some_and(|message| message.contains("incomplete inventory"))
            );
            assert!(failure.error().api_error().is_none());
        } else {
            assert!(
                failure
                    .error()
                    .detail::<crate::wire::product::operation::RefusedGuard>()
                    .is_some()
            );
            assert_eq!(
                failure.error().api_error().unwrap().code(),
                "InvalidResponse"
            );
        }
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn paid_create_rechecks_acknowledgement_after_confirmation_bearer_changes() {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(
            "202 Accepted",
            r#"{"data":{"uuid":"volume-1"},"unknown":"confirmation-bearer"}"#,
        ),
        response(&AVAILABLE.replace("AVAILABLE", "CREATING")),
        response(AVAILABLE),
    ]);
    let clock = crate::testing::clock_advancing_on_sleep(
        crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
        std::time::Duration::from_secs(2),
    );
    let options = crate::ConfigOptions::default()
        .with_clock(Arc::new(clock))
        .with_credentials_provider(provider([
            "initial-bearer",
            "initial-bearer",
            "initial-bearer",
            "confirmation-bearer",
        ]));
    let failure = volume_with_options(&server, options, true)
        .create_volume(&RequestContext::new().0, create())
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 3);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
    assert!(failure.partial().is_none());
    assert_eq!(
        failure.recovery_action(),
        Some(crate::RecoveryAction::CheckExistingVolume)
    );
}

#[tokio::test]
async fn duplicate_and_complete_inventory_decisions_recheck_earlier_pages() {
    let early = r#"{"listData":[{"uuid":"volume-2","name":"prior-page-secret"}],"page":1,"pageSize":1,"totalPage":2,"totalItem":2}"#;
    for name in [create().name, "other".to_owned()] {
        let final_page = serde_json::json!({"listData":[{"uuid":"volume-3","name":name}],"page":2,"pageSize":1,"totalPage":2,"totalItem":2});
        let server = Server::start(vec![
            response(early),
            response(&final_page.to_string()),
            response(QUOTE),
        ]);
        let options = crate::ConfigOptions::default().with_credentials_provider(provider([
            "toy-first",
            "prior-page-secret",
            "prior-page-secret",
        ]));
        let failure = volume_with_options(&server, options, true)
            .create_volume(&RequestContext::new().0, create())
            .await
            .unwrap_err();
        assert_eq!(server.requests(), 2);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert!(
            failure
                .error()
                .detail::<crate::wire::product::operation::RefusedGuard>()
                .is_some()
        );
    }
}

#[tokio::test]
async fn deadline_at_reflected_acknowledgement_keeps_no_repeat_and_cause() {
    let server = Server::start(vec![
        response(EMPTY),
        response(QUOTE),
        reply(
            "202 Accepted",
            r#"{"data":{"uuid":"volume-1"},"unknown":"confirmation-bearer"}"#,
        ),
        response(&AVAILABLE.replace("AVAILABLE", "CREATING")),
    ]);
    let clock = crate::testing::ResponseBodyClock::new()
        .on_volume_confirmation(|clock| clock.advance(std::time::Duration::from_secs(300)));
    let options = crate::ConfigOptions::default()
        .with_clock(Arc::new(clock))
        .with_credentials_provider(provider([
            "initial-bearer",
            "initial-bearer",
            "initial-bearer",
            "confirmation-bearer",
        ]));
    let failure = volume_with_options(&server, options, true)
        .create_volume(&RequestContext::new().0, create())
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 3);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert!(failure.error().has_identity(ErrorClass::DeadlineExceeded));
    assert!(failure.partial().is_none());
    assert_eq!(
        failure.recovery_action(),
        Some(crate::RecoveryAction::DoNotRepeatVolumeCreate)
    );
}

#[tokio::test]
async fn redirected_volume_inventory_cannot_authorize_a_create() {
    let server = Server::start(vec![
        Response::Http {
            status: "307 Temporary Redirect".into(),
            headers: vec![
                (
                    "Location".into(),
                    "/server/v2/project-1/volumes?name=other".into(),
                ),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response(EMPTY),
        response(QUOTE),
        reply("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
    ]);
    let mut input = create();
    input.no_wait = true;
    let failure = volume_with_options(
        &server,
        crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-bearer")),
        true,
    )
    .create_volume(&RequestContext::new().0, input)
    .await
    .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn no_wait_volume_delete_refuses_transport_reflection_in_accepted_body() {
    let server = Server::start(vec![
        response(AVAILABLE),
        reply(ACCEPTED, r#"{"ignored":"toy-bearer"}"#),
    ]);
    let options =
        crate::ConfigOptions::default().with_static_token(crate::Secret::new("toy-bearer"));
    let failure = volume_with_options(&server, options, true)
        .delete_volume(
            &RequestContext::new().0,
            DeleteVolumeInput {
                volume_id: "volume-1".into(),
                no_wait: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(server.requests(), 2);
}
