use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{
    CreateSyntheticQuotaInput, EnsureSyntheticQuotaInput, ErrorDetail, RecoverSyntheticQuotaInput,
    RenewSyntheticQuotaInput,
};
use greennode_sdk::testing::{Response, Server};
use greennode_sdk::{RequestContext, WriteDispatch};

const ABSENT: &str = include_str!("../fixtures/monitor/synthetic_quote_absent.json");
const ACTIVE: &str = include_str!("../fixtures/monitor/synthetic_quote_renewal.json");
const DETAIL: &str = include_str!("../fixtures/monitor/quota_quota_detail.json");
const CLASSES: &str = include_str!("../fixtures/monitor/quota_classes.json");
const UNITS: &str = include_str!("../fixtures/monitor/quota_class_packages.json");
const SETTINGS: &str = include_str!("../fixtures/monitor/quota_settings.json");
const FREE: &str = r#"{"optimumPrice":0}"#;
const ACK: &str = r#"{"orderId":"synth-order"}"#;

fn catalog(settings: &str) -> Vec<Response> {
    [CLASSES, UNITS, "[]", settings]
        .map(|body| response("200 OK", body))
        .into()
}
fn change_plan(before: &str, fresh: &str, settings: &str) -> Vec<Response> {
    let mut responses = vec![response("200 OK", before), response("200 OK", DETAIL)];
    responses.extend(catalog(SETTINGS));
    responses.push(response("200 OK", FREE));
    responses.extend(catalog(settings));
    responses.extend([response("200 OK", fresh), response("200 OK", DETAIL)]);
    responses
}

#[tokio::test]
async fn accepted_synthetic_create_is_reconciled_without_a_second_order() {
    let mut replies = vec![response("404 Not Found", ABSENT)];
    replies.extend(catalog(SETTINGS));
    replies.push(response("200 OK", FREE));
    replies.extend(catalog(SETTINGS));
    replies.extend([response("404 Not Found", ABSENT), response("200 OK", ACK)]);
    let created = ACTIVE.replace("2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    replies.extend([response("200 OK", &created), response("200 OK", DETAIL)]);
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let mut input = CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    input.no_wait = true;
    let first = client
        .create_synthetic_quota(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(first.outcome.sent, "yes");
    assert_eq!(first.outcome.stage, "accepted");
    assert!(!first.created);
    let confirmed = client.create_synthetic_quota(&ctx, input).await.unwrap();
    assert_eq!(confirmed.outcome.stage, "confirmed");
    assert!(confirmed.created);
    assert_eq!(confirmed.quota.unwrap().id, "quota-1");
    assert_eq!(server.requests(), 14);
    assert!(server.heads()[11].starts_with(b"POST /billing-api/v2/synthetic/quota "));
}

#[tokio::test]
async fn early_renewal_returns_not_due_without_quoting_or_sending() {
    let mut replies = vec![response("200 OK", ACTIVE), response("200 OK", DETAIL)];
    replies.extend(catalog(SETTINGS));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.max_days_left = 7;
    let output = client(&server)
        .renew_synthetic_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.outcome.sent, "no");
    assert_eq!(output.outcome.stage, "not-due");
    assert!(output.previous_end_time.is_some());
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn recovery_requires_expired_trash_and_uses_the_trash_order_endpoint() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-", "2000-");
    let mut replies = change_plan(&trash, &trash, SETTINGS);
    replies.push(response("200 OK", ACK));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let mut input = RecoverSyntheticQuotaInput::new("quota-1");
    input.no_wait = true;
    let output = client(&server)
        .recover_synthetic_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.outcome.stage, "accepted");
    assert!(!output.recovered);
    assert_eq!(
        target(&server.heads()[13]).path(),
        "/billing-api/v2/trash/synthetic/quota"
    );
}

#[tokio::test]
async fn ensure_refuses_when_the_final_catalog_changes_after_the_quote() {
    let settings = SETTINGS.replace("PRE-PAID", "POST-PAID");
    let mut replies = change_plan(ACTIVE, ACTIVE, &settings);
    replies.push(response("200 OK", ACK));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let input = EnsureSyntheticQuotaInput {
        expected_quota_id: "quota-1".into(),
        no_wait: true,
        ..Default::default()
    };
    let failure = client(&server)
        .ensure_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let result = failure.partial().unwrap();
    assert_eq!(result.action, "renew");
    assert_eq!(result.outcome.stage, "preflight");
    assert_eq!(server.requests(), 13);
}

#[tokio::test]
async fn final_quota_replacement_prevents_renewal() {
    let fresh = ACTIVE.replace("quota-1", "quota-2");
    let mut replies = change_plan(ACTIVE, &fresh, SETTINGS);
    let fresh_detail = DETAIL.replace("quota-1", "quota-2");
    *replies.last_mut().unwrap() = response("200 OK", &fresh_detail);
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(
            &ctx,
            RenewSyntheticQuotaInput::new("2099-02-01T0:00:00,000Z"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 13);
}

#[tokio::test]
async fn create_requires_the_exact_source_absence_response() {
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"message":"not found"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.partial().unwrap().outcome.stage, "preflight");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn pending_payment_preserves_send_evidence_until_a_different_intent_reconciles() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    replies.extend([
        response(
            "200 OK",
            r#"{"orderId":"synth-order","paymentUrl":"https://billing.example.test/pay"}"#,
        ),
        response("200 OK", &extended),
        response("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.no_wait = true;
    let failure = client.renew_synthetic_quota(&ctx, input).await.unwrap_err();
    let result = failure.partial().unwrap();
    assert!(result.payment_pending);
    assert_eq!(result.outcome.sent, "yes");
    assert_eq!(result.outcome.stage, "unconfirmed");
    let (expired, _) = RequestContext::new();
    let expired = expired.child_with_deadline(tokio::time::Instant::now());
    let input = EnsureSyntheticQuotaInput {
        expected_quota_id: "quota-1".into(),
        ..Default::default()
    };
    let failure = client
        .ensure_synthetic_quota(&expired, input.clone())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::DeadlineExceeded)
    );
    assert_eq!(failure.partial().unwrap().outcome.sent, "yes");
    assert_eq!(server.requests(), 14);
    let failure = client
        .ensure_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.partial().unwrap().outcome.stage, "preflight");
    assert_eq!(server.requests(), 16);
}

#[tokio::test]
async fn recovery_refuses_a_zero_expiry() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-02-01T07:00:00+07:00", "0001-01-01T00:00:00Z");
    let server = Server::start(vec![response("200 OK", &trash), response("200 OK", DETAIL)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .recover_synthetic_quota(&ctx, RecoverSyntheticQuotaInput::new("quota-1"))
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::InvalidInput)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn reflected_order_id_does_not_erase_pending_payment() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response(
        "200 OK",
        r#"{"orderId":"toy-management","paymentUrl":"https://billing.example.test/pay"}"#,
    ));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let partial = failure.partial().unwrap();
    assert!(partial.payment_pending);
    assert_eq!(partial.order_id, "<redacted>");
}

#[tokio::test]
async fn oversized_order_keeps_the_observed_status() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response("200 OK", &"x".repeat((1 << 20) + 1)));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let outcome = &failure.partial().unwrap().outcome;
    assert_eq!(
        failure.error().api_error().map(|api| api.code()),
        Some("ResponseTooLarge")
    );
    assert_eq!(outcome.sent, "yes");
    assert_eq!(
        outcome.error.as_ref().unwrap().status,
        greennode_sdk::monitor::ResponseField::Value(200)
    );
}

#[tokio::test]
async fn different_auto_renew_guard_reconciles_pending_order_before_refusing() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    replies.extend([
        response("200 OK", ACK),
        response("200 OK", &extended),
        response("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut renew = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    renew.no_wait = true;
    client.renew_synthetic_quota(&ctx, renew).await.unwrap();
    let mut auto_renew = greennode_sdk::monitor::SetSyntheticQuotaAutoRenewInput::new(true);
    auto_renew.expected_quota_id = "quota-other".into();
    let failure = client
        .set_synthetic_quota_auto_renew(&ctx, auto_renew)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(server.requests(), 16);
}

#[tokio::test]
async fn cross_operation_reconciliation_keeps_the_callers_operation() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.extend([
        response("200 OK", ACK),
        response("403 Forbidden", r#"{"message":"ignored"}"#),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut renew = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    renew.no_wait = true;
    client.renew_synthetic_quota(&ctx, renew).await.unwrap();
    let failure = client
        .ensure_synthetic_quota(&ctx, EnsureSyntheticQuotaInput::default())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.operation(), "monitor.EnsureSyntheticQuota");
    assert_eq!(api.status(), Some(403));
    assert_eq!(server.requests(), 15);
}
