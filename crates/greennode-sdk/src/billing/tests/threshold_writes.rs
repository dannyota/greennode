use super::*;

const THRESHOLD_ID: &str = "threshold-1";
const CREATED: &[u8] = include_bytes!("../fixtures/CreateBudgetThreshold.json");
const ADVICE: &str = "budget write outcome is unconfirmed; read current state before retrying";

fn create_input() -> CreateBudgetThresholdInput {
    CreateBudgetThresholdInput::new(BUDGET_ID, "ACTUAL", 100)
}

#[tokio::test]
async fn threshold_writes_send_baseline_requests_and_decode_success() {
    let server = Server::start(vec![
        response("201 Created", CREATED),
        response("201 Created", CREATED),
        response(
            "200 OK",
            include_bytes!("../fixtures/UpdateBudgetThreshold.json"),
        ),
        response(
            "200 OK",
            include_bytes!("../fixtures/DeleteBudgetThreshold.json"),
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);

    let output = client
        .create_budget_threshold(&ctx, create_input())
        .await
        .unwrap();
    assert_eq!(output.threshold.uuid, THRESHOLD_ID);
    assert_eq!(output.threshold.threshold_percentage, 100.0);
    assert!(output.threshold.enabled);
    assert!(output.threshold.last_alert_at.is_empty());
    client
        .create_budget_threshold(
            &ctx,
            CreateBudgetThresholdInput {
                max_alerts_per_period: 3,
                reminder_interval_hours: 12,
                ..create_input()
            },
        )
        .await
        .unwrap();
    client
        .update_budget_threshold(
            &ctx,
            UpdateBudgetThresholdInput {
                enabled: Some(false),
                max_alerts_per_period: Some(0),
                ..UpdateBudgetThresholdInput::new(BUDGET_ID, THRESHOLD_ID)
            },
        )
        .await
        .unwrap();
    client
        .delete_budget_threshold(
            &ctx,
            DeleteBudgetThresholdInput::new(BUDGET_ID, THRESHOLD_ID),
        )
        .await
        .unwrap();

    assert_eq!(server.requests(), 4);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"POST /gateway/api/v1/budgets/budget-1/thresholds "));
    assert!(heads[1].starts_with(b"POST /gateway/api/v1/budgets/budget-1/thresholds "));
    assert!(heads[2].starts_with(b"PUT /gateway/api/v1/budgets/budget-1/thresholds/threshold-1 "));
    assert!(
        heads[3].starts_with(b"DELETE /gateway/api/v1/budgets/budget-1/thresholds/threshold-1 ")
    );
    let bodies = server.bodies();
    for (body, alerts, hours) in [(&bodies[0], 1, 648), (&bodies[1], 3, 12)] {
        assert!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap()
                == serde_json::json!({
                    "thresholdType": "ACTUAL", "thresholdPercentage": 100,
                    "comparisonOperator": "GTE", "maxAlertsPerPeriod": alerts,
                    "reminderIntervalHours": hours,
                })
        );
    }
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bodies[2]).unwrap()
            == serde_json::json!({"enabled": false, "maxAlertsPerPeriod": 0})
    );
    assert!(bodies[3].is_empty());
}

#[tokio::test]
async fn invalid_threshold_writes_are_not_sent() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for input in [
        CreateBudgetThresholdInput::new("", "ACTUAL", 100),
        CreateBudgetThresholdInput::new("../budget", "ACTUAL", 100),
        CreateBudgetThresholdInput::new(BUDGET_ID, "", 100),
        CreateBudgetThresholdInput::new(BUDGET_ID, "ACTUAL", 0),
    ] {
        let error = client
            .create_budget_threshold(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    }
    for input in [
        UpdateBudgetThresholdInput::new(BUDGET_ID, THRESHOLD_ID),
        UpdateBudgetThresholdInput {
            enabled: Some(false),
            ..UpdateBudgetThresholdInput::new(BUDGET_ID, ".")
        },
    ] {
        let error = client
            .update_budget_threshold(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    }
    for input in [
        DeleteBudgetThresholdInput::new("a/b", THRESHOLD_ID),
        DeleteBudgetThresholdInput::new(BUDGET_ID, ""),
    ] {
        let error = client
            .delete_budget_threshold(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
        assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    }
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn threshold_create_keeps_dispatch_and_classifies_truncation_by_status() {
    let (ctx, _) = RequestContext::new();
    for (http, status, code, advice) in [
        ("201 Created", 201, "InvalidResponse", true),
        ("403 Forbidden", 403, "Forbidden", false),
        ("503 Service Unavailable", 503, "ServerError", true),
        ("401 Unauthorized", 401, "Unauthorized", false),
    ] {
        let server = Server::start(vec![
            Response::RawClose(
                format!("HTTP/1.1 {http}\r\nContent-Length: 256\r\n\r\n{{\"message\":\"{BEARER}")
                    .into_bytes(),
            ),
            response("201 Created", CREATED),
        ]);
        let error = client(&server, BEARER)
            .create_budget_threshold(&ctx, create_input())
            .await
            .unwrap_err();
        assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(error.partial().is_none());
        assert!(!format!("{error:?} {}", error.error()).contains(BEARER));
        let api = error.error().api_error().unwrap();
        assert_eq!(api.operation(), "billing.CreateBudgetThreshold");
        assert_eq!(api.status(), Some(status));
        assert_eq!(api.code(), code);
        assert_eq!(api.message() == ADVICE, advice);
        assert!(!api.retryable());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn threshold_create_withholds_invalid_success_bodies() {
    let (ctx, _) = RequestContext::new();
    for (bearer, body) in [
        (
            BEARER,
            &br#"{"code":201,"data":{"thresholdType":"ACTUAL"}}"#[..],
        ),
        (
            BEARER,
            &br#"{"code":201,"data":{"uuid":"threshold-1","enabled":false,"Enabled":true}}"#[..],
        ),
        (BEARER, &br#"{"code":201,"data":{"uuid":"threshold-1""#[..]),
        (
            "12345678",
            &br#"{"code":201,"data":{"uuid":"threshold-1","maxAlertsPerPeriod":12345678}}"#[..],
        ),
    ] {
        let server = Server::start(vec![response("201 Created", body)]);
        let error = client(&server, bearer)
            .create_budget_threshold(&ctx, create_input())
            .await
            .unwrap_err();
        assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(error.partial().is_none());
        assert!(!format!("{error:?} {}", error.error()).contains(bearer));
        let api = error.error().api_error().unwrap();
        assert_eq!(api.status(), Some(201));
        assert_eq!(api.code(), "InvalidResponse");
        assert_eq!(api.message(), ADVICE);
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn threshold_delete_retains_not_found_identity_and_dispatch() {
    let server = Server::start(vec![response(
        "400 Bad Request",
        include_bytes!("../fixtures/ThresholdNotFound.json"),
    )]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, BEARER)
        .delete_budget_threshold(
            &ctx,
            DeleteBudgetThresholdInput::new(BUDGET_ID, THRESHOLD_ID),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(400));
    assert_eq!(api.code(), "NotFound");
    assert_eq!(api.message(), "Threshold not found: threshold-1");
    assert_eq!(server.requests(), 1);
}
