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
