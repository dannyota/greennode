use super::*;
use greennode_sdk::ErrorClass;
use greennode_sdk::monitor::{DeleteSyntheticQuotaInput, SetSyntheticQuotaAutoRenewInput};

#[tokio::test]
async fn renewal_with_matching_expiry_reconciles_delete_then_rejects_changed_intent() {
    let server = Server::start(vec![
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("503 Service Unavailable", "{}"),
        response("404 Not Found", ABSENT),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .delete_synthetic_quota(&ctx, DeleteSyntheticQuotaInput::new("quota-1"))
            .await
            .is_err()
    );
    let failure = client
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_ne!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn renewal_keeps_previous_expiry_when_auto_renew_settlement_fails() {
    let server = Server::start(vec![
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("200 OK", FREE),
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("503 Service Unavailable", "{}"),
        response("403 Forbidden", "{}"),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .set_synthetic_quota_auto_renew(&ctx, SetSyntheticQuotaAutoRenewInput::new(true))
            .await
            .is_err()
    );
    let failure = client
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    let output = failure.partial().unwrap();
    let previous = output.previous_end_time.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(previous).unwrap(),
        "2099-02-01T07:00:00+07:00"
    );
    assert_ne!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn auto_renew_to_renew_matches_the_complete_go_partial() {
    let server = Server::start(vec![
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("200 OK", FREE),
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("503 Service Unavailable", "{}"),
        response("403 Forbidden", "{}"),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    assert!(
        client
            .set_synthetic_quota_auto_renew(&ctx, SetSyntheticQuotaAutoRenewInput::new(true))
            .await
            .is_err()
    );
    let failure = client
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let got = serde_json::to_value(failure.partial().unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/monitor/renew-after-auto-renew.json"
    ))
    .unwrap();
    assert!(got == expected);
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn standalone_order_to_ensure_matches_the_complete_go_partials() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-", "2000-");
    for (recover, present, fixture) in [
        (
            false,
            ACTIVE,
            include_str!("../../fixtures/monitor/ensure-after-renew.json"),
        ),
        (
            true,
            trash.as_str(),
            include_str!("../../fixtures/monitor/ensure-after-recover.json"),
        ),
    ] {
        let mut replies = change_plan(present, present, SETTINGS);
        replies.extend([response("200 OK", ACK), response("403 Forbidden", "{}")]);
        let server = Server::start(replies);
        let client = client(&server);
        let (ctx, _) = RequestContext::new();
        if recover {
            let mut input = RecoverSyntheticQuotaInput::new("quota-1");
            input.no_wait = true;
            client.recover_synthetic_quota(&ctx, input).await.unwrap();
        } else {
            let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
            input.no_wait = true;
            client.renew_synthetic_quota(&ctx, input).await.unwrap();
        }
        let input = EnsureSyntheticQuotaInput {
            expected_quota_id: "quota-1".into(),
            ..Default::default()
        };
        let failure = client
            .ensure_synthetic_quota(&ctx, input)
            .await
            .unwrap_err();
        let got = serde_json::to_value(failure.partial().unwrap()).unwrap();
        let expected: serde_json::Value = serde_json::from_str(fixture).unwrap();
        assert!(got == expected);
        assert_eq!(server.requests(), 15);
    }
}
