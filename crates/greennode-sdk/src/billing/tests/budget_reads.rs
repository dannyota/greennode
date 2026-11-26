use super::*;

const COST: &[u8] = include_bytes!("../fixtures/GetCurrentPeriodCost.json");
const THRESHOLDS: &[u8] = include_bytes!("../fixtures/ListBudgetThresholds.json");
const ALERTS: &[u8] = include_bytes!("../fixtures/ListBudgetAlerts.json");

#[tokio::test]
async fn current_period_cost_reads_account_cost() {
    let server = Server::start(vec![response("200 OK", COST)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .get_current_period_cost(&ctx, GetCurrentPeriodCostInput::default())
        .await
        .unwrap();
    assert_request(&server, b"GET /gateway/api/v1/budgets/cost/overview ");
    assert_eq!(output.period_cost.period_key, "2026-09");
    assert_eq!(output.period_cost.actual_cost, 4_200_000.0);
    assert_eq!(output.period_cost.forecasted_cost, 5_100_000.0);
    assert_eq!(
        serde_json::to_value(output).unwrap()["PeriodCost"]["ActualCost"],
        4_200_000.0,
    );
}

#[tokio::test]
async fn list_budget_thresholds_reads_nullable_fields_and_ignores_metadata() {
    let server = Server::start(vec![response("200 OK", THRESHOLDS)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .list_budget_thresholds(&ctx, ListBudgetThresholdsInput::new(BUDGET_ID))
        .await
        .unwrap();
    assert_request(&server, b"GET /gateway/api/v1/budgets/budget-1/thresholds ");
    let [threshold] = output.items.as_slice() else {
        panic!("expected one threshold");
    };
    assert_eq!(threshold.uuid, "threshold-1");
    assert_eq!(threshold.threshold_percentage, 100.0);
    assert!(!threshold.enabled);
    assert_eq!(threshold.reminder_interval_hours, 24);
    assert!(threshold.last_alert_at.is_empty());
    assert_eq!(
        serde_json::to_value(output).unwrap()["Items"][0]["UUID"],
        "threshold-1",
    );
}

#[tokio::test]
async fn list_budget_alerts_sends_optional_encoded_period_and_decodes_recipients() {
    let (ctx, _) = RequestContext::new();
    for (period, path) in [
        (
            "2026-09 &",
            &b"GET /gateway/api/v1/budgets/budget-1/alerts?periodKey=2026-09+%26 "[..],
        ),
        ("", &b"GET /gateway/api/v1/budgets/budget-1/alerts "[..]),
    ] {
        let server = Server::start(vec![response("200 OK", ALERTS)]);
        let mut input = ListBudgetAlertsInput::new(BUDGET_ID);
        input.period_key = period.into();
        let output = client(&server, BEARER)
            .list_budget_alerts(&ctx, input)
            .await
            .unwrap();
        assert_request(&server, path);
        let [sent, failed] = output.items.as_slice() else {
            panic!("expected two alerts");
        };
        assert_eq!(sent.recipients.as_deref(), Some(&["<account>".into()][..]));
        assert!(sent.recipients_raw.is_empty());
        assert_eq!(failed.delivery_status, "FAILED");
        assert!(failed.recipients.is_none());
        assert_eq!(failed.recipients_raw, "not-an-array");
    }
}

fn assert_request(server: &Server, path: &[u8]) {
    assert_eq!(server.requests(), 1);
    let heads = server.heads();
    assert!(heads[0].starts_with(path));
    assert!(String::from_utf8_lossy(&heads[0]).contains(&format!("Bearer {BEARER}")));
    assert!(server.bodies().iter().all(Vec::is_empty));
}

#[tokio::test]
async fn budget_lists_refuse_invalid_ids_without_connecting() {
    let server = Server::start(vec![]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    for id in ["", "a/b"] {
        let thresholds = client
            .list_budget_thresholds(&ctx, ListBudgetThresholdsInput::new(id))
            .await
            .unwrap_err();
        let alerts = client
            .list_budget_alerts(&ctx, ListBudgetAlertsInput::new(id))
            .await
            .unwrap_err();
        assert_eq!(thresholds.error().class(), ErrorClass::InvalidInput);
        assert_eq!(alerts.error().class(), ErrorClass::InvalidInput);
    }
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn budget_read_errors_keep_status_and_identity() {
    let server = Server::start(vec![
        response(
            "403 Forbidden",
            br#"{"code":403,"message":"Forbidden","data":null}"#,
        ),
        response(
            "400 Bad Request",
            include_bytes!("../fixtures/BudgetNotFound.json"),
        ),
        response(
            "200 OK",
            include_bytes!("../fixtures/ThresholdNotFound.json"),
        ),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let cost = client
        .get_current_period_cost(&ctx, GetCurrentPeriodCostInput::default())
        .await
        .unwrap_err();
    let thresholds = client
        .list_budget_thresholds(&ctx, ListBudgetThresholdsInput::new(BUDGET_ID))
        .await
        .unwrap_err();
    let alerts = client
        .list_budget_alerts(&ctx, ListBudgetAlertsInput::new(BUDGET_ID))
        .await
        .unwrap_err();
    for (error, code, status) in [
        (cost.error(), "Forbidden", 403),
        (thresholds.error(), "NotFound", 400),
        (alerts.error(), "NotFound", 200),
    ] {
        let api = error.api_error().unwrap();
        assert_eq!(api.code(), code);
        assert_eq!(api.status(), Some(status));
    }
    assert!(thresholds.error().has_identity(ErrorClass::NotFound));
    assert!(alerts.error().has_identity(ErrorClass::NotFound));
}

#[tokio::test]
async fn current_period_cost_requires_both_costs_and_rejects_reflection_after_rounding() {
    let server = Server::start(vec![
        response("200 OK", br#"{"code":200,"data":{"actualCost":0}}"#),
        response(
            "200 OK",
            br#"{"code":200,"data":{"actualCost":null,"forecastedCost":0}}"#,
        ),
        response(
            "200 OK",
            br#"{"code":200,"data":{"actualCost":0.0000001,"forecastedCost":0}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    for bearer in [BEARER, BEARER, "e-7"] {
        let failure = client(&server, bearer)
            .get_current_period_cost(&ctx, GetCurrentPeriodCostInput::default())
            .await
            .unwrap_err();
        assert!(!format!("{failure:?} {}", failure.error()).contains(bearer));
        assert_invalid_response(failure);
    }
}

#[tokio::test]
async fn current_period_cost_accepts_an_unused_numeric_spelling() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":200,"data":{"actualCost":1e6,"forecastedCost":0}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, "e+06")
        .get_current_period_cost(&ctx, GetCurrentPeriodCostInput::default())
        .await
        .unwrap();
    assert_eq!(output.period_cost.actual_cost, 1_000_000.0);
}

#[tokio::test]
async fn budget_lists_accept_empty_arrays_and_reject_null_rows() {
    let empty = br#"{"code":200,"data":[]}"#;
    let null_row = br#"{"code":200,"data":[null]}"#;
    let server = Server::start(vec![
        response("200 OK", empty),
        response("200 OK", empty),
        response("200 OK", null_row),
        response("200 OK", null_row),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let thresholds = ListBudgetThresholdsInput::new(BUDGET_ID);
    let alerts = ListBudgetAlertsInput::new(BUDGET_ID);
    let output = client
        .list_budget_thresholds(&ctx, thresholds.clone())
        .await
        .unwrap();
    assert!(output.items.is_empty());
    let output = client
        .list_budget_alerts(&ctx, alerts.clone())
        .await
        .unwrap();
    assert!(output.items.is_empty());
    assert_invalid_response(
        client
            .list_budget_thresholds(&ctx, thresholds)
            .await
            .unwrap_err(),
    );
    assert_invalid_response(client.list_budget_alerts(&ctx, alerts).await.unwrap_err());
}

#[tokio::test]
async fn budget_alerts_refuse_credentials_escaped_inside_recipients() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":200,"data":[{"recipients":"[\"\\u0073ynthetic-billing-bearer\"]"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .list_budget_alerts(&ctx, ListBudgetAlertsInput::new(BUDGET_ID))
        .await
        .unwrap_err();
    assert_invalid_response(failure);
}
