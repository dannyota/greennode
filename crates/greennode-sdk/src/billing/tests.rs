mod budget_reads;
mod budget_writes;
mod threshold_writes;

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, ErrorClass, Secret, WriteDispatch};

const BEARER: &str = "synthetic-billing-bearer";
const BUDGET_ID: &str = "budget-1";

fn response(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn client(server: &Server, bearer: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(bearer))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(server.origin.clone()),
                vserver: Some(server.origin.clone()),
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
async fn list_sends_summary_and_status() {
    let server = Server::start(vec![response(
        "200 OK",
        include_bytes!("fixtures/ListBudgetsSummary.json"),
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .list_budgets(
            &ctx,
            ListBudgetsInput {
                status: "PAUSED".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(server.requests(), 1);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /gateway/api/v1/budgets?status=PAUSED&view=summary "));
    assert!(String::from_utf8_lossy(&heads[0]).contains(&format!("Bearer {BEARER}")));
    let [item] = output.items.as_slice() else {
        panic!("expected one budget");
    };
    assert_eq!(item.uuid, BUDGET_ID);
    assert_eq!(item.limit_amount, 1_000_000);
    assert_eq!(item.actual_cost, Some(0.0));
    assert_eq!(item.threshold_percentage, None);
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Items"][0]["LimitAmount"], 1_000_000);
    assert!(json["Items"][0]["ThresholdPercentage"].is_null());
}

#[tokio::test]
async fn list_returns_empty_items_and_omits_empty_filter() {
    let server = Server::start(vec![response(
        "200 OK",
        include_bytes!("fixtures/ListBudgets.json"),
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .list_budgets(&ctx, ListBudgetsInput::default())
        .await
        .unwrap();
    assert!(output.items.is_empty());
    assert!(server.heads()[0].starts_with(b"GET /gateway/api/v1/budgets?view=summary "));
    assert_eq!(serde_json::to_string(&output).unwrap(), r#"{"Items":[]}"#);
}

#[tokio::test]
async fn get_reads_budget_by_id() {
    let server = Server::start(vec![response(
        "200 OK",
        include_bytes!("fixtures/GetBudget.json"),
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .clone()
        .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
        .await
        .unwrap();
    assert_eq!(server.requests(), 1);
    assert!(server.heads()[0].starts_with(b"GET /gateway/api/v1/budgets/budget-1 "));
    assert_eq!(output.budget.uuid, BUDGET_ID);
    assert_eq!(output.budget.name, "example-budget");
    assert_eq!(output.budget.created_at, "2026-09-01T00:00:00");
    assert_eq!(output.budget.actual_cost, None);
    assert_eq!(
        serde_json::to_value(output).unwrap()["Budget"]["UUID"],
        BUDGET_ID
    );
}

#[tokio::test]
async fn get_refuses_invalid_id_before_sending() {
    let server = Server::start(Vec::new());
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    for id in ["", "../budget-1"] {
        let failure = client
            .get_budget(&ctx, GetBudgetInput::new(id))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
        assert!(failure.partial().is_none());
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn api_errors_preserve_status_and_only_safe_fields() {
    let error_body = format!(r#"{{"code":"Denied","message":"rejected {BEARER}","data":null}}"#);
    let forbidden =
        format!(r#"{{"code":"Denied","message":"Budget not found: {BEARER}","data":null}}"#);
    let server = Server::start(vec![
        response("200 OK", error_body.as_bytes()),
        response("403 Forbidden", forbidden.as_bytes()),
        response("404 Not Found", b"{}"),
        response("401 Unauthorized", b"{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for (class, status, code) in [
        (ErrorClass::Api, 200, "Denied"),
        (ErrorClass::PermissionDenied, 403, "Denied"),
        (ErrorClass::NotFound, 404, "NotFound"),
        (ErrorClass::Auth, 401, "Unauthorized"),
    ] {
        let failure = client
            .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        assert_eq!(failure.error().class(), class);
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.operation(), "billing.GetBudget");
        assert_eq!(api.status(), Some(status));
        assert!(api.code() == code);
        assert!(!format!("{failure:?} {}", failure.error()).contains(BEARER));
    }
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn budget_not_found_envelope_keeps_observed_http_status() {
    let body = include_bytes!("fixtures/BudgetNotFound.json");
    let server = Server::start(vec![
        response("400 Bad Request", body),
        response("200 OK", body),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for expected_status in [400, 200] {
        let failure = client
            .get_budget(&ctx, GetBudgetInput::new("budget-x"))
            .await
            .unwrap_err();
        assert!(failure.error().has_identity(ErrorClass::NotFound));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(expected_status));
        assert!(api.code() == "NotFound");
    }
}

#[tokio::test]
async fn reflected_credentials_are_refused() {
    let echo = BEARER.replacen('s', r#"\u0073"#, 1);
    let body = format!(r#"{{"code":200,"data":{{"uuid":"budget-1","name":"{echo}"}}}}"#);
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let (ctx, _) = RequestContext::new();
    let Err(failure) = client(&server, BEARER)
        .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
        .await
    else {
        panic!("credential reflection returned a budget");
    };
    assert_invalid_response(failure);
}

#[tokio::test]
async fn rounded_numeric_fields_cannot_reflect_credentials() {
    for (fragment, cost) in [("e-7", "0.0000001"), ("e+21", "1000000000000000000000")] {
        let body = format!(
            r#"{{"code":200,"data":{{"uuid":"budget-1","limitAmount":1,"actualCost":{cost}}}}}"#
        );
        let server = Server::start(vec![response("200 OK", body.as_bytes())]);
        let (ctx, _) = RequestContext::new();
        let Err(failure) = client(&server, fragment)
            .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
            .await
        else {
            panic!("rendered credential reflection returned a budget");
        };
        assert!(!format!("{failure:?} {}", failure.error()).contains(fragment));
        assert_invalid_response(failure);
    }
}

#[tokio::test]
async fn an_unused_numeric_spelling_does_not_withhold_a_budget() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":200,"data":{"uuid":"budget-1","limitAmount":1,"actualCost":1e6}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, "e+06")
        .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
        .await
        .unwrap();
    assert_eq!(result.budget.actual_cost, Some(1_000_000.0));
}

#[tokio::test]
async fn get_requires_nonblank_uuid_and_list_requires_rows() {
    let server = Server::start(vec![
        response("200 OK", br#"{"code":200,"data":{"uuid":"  "}}"#),
        response("200 OK", br#"{"code":200,"data":[null]}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    assert_invalid_response(
        client
            .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
            .await
            .unwrap_err(),
    );
    assert_invalid_response(
        client
            .list_budgets(&ctx, ListBudgetsInput::default())
            .await
            .unwrap_err(),
    );
}

#[tokio::test]
async fn malformed_success_envelopes_are_refused() {
    let server = Server::start(
        [
            br#"{"data":[]}"#.as_slice(),
            br#"{"code":"200","data":[]}"#,
            br#"{"code":200,"data":null}"#,
            br#"{"code":200,"data":[{"limitAmount":1,"LimitAmount":2}]}"#,
        ]
        .into_iter()
        .map(|body| response("200 OK", body))
        .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for _ in 0..4 {
        assert_invalid_response(
            client
                .list_budgets(&ctx, ListBudgetsInput::default())
                .await
                .unwrap_err(),
        );
    }
}

#[tokio::test]
async fn limit_amount_preserves_exact_integers() {
    let cases = [
        ("9007199254740993", Some(9_007_199_254_740_993)),
        ("9223372036854775807.0", Some(i64::MAX)),
        ("-9223372036854775808", Some(i64::MIN)),
        ("1.20e3", Some(1_200)),
        ("120e-1", Some(12)),
        ("9223372036854775808", None),
        ("1.00000000000000001", None),
        ("1e999999999", None),
        ("null", None),
    ];
    let server = Server::start(
        cases
            .iter()
            .map(|(amount, _)| {
                let body = format!(
                    r#"{{"code":null,"data":{{"uuid":"budget-1","limitAmount":{amount}}}}}"#
                );
                response("200 OK", body.as_bytes())
            })
            .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for (_, expected) in cases {
        let result = client
            .get_budget(&ctx, GetBudgetInput::new(BUDGET_ID))
            .await;
        if let Some(amount) = expected {
            assert_eq!(result.unwrap().budget.limit_amount, amount);
        } else {
            assert_invalid_response(result.unwrap_err());
        }
    }
}

#[tokio::test]
async fn oversized_success_keeps_status_and_limit_error() {
    let body = vec![b' '; (8 << 20) + 1];
    let server = Server::start(vec![response("200 OK", &body)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .list_budgets(&ctx, ListBudgetsInput::default())
        .await
        .unwrap_err();
    assert!(failure.error().is_body_too_large());
    assert_invalid_response(failure);
}

fn assert_invalid_response<T>(failure: Failure<T>) {
    assert!(failure.partial().is_none());
    assert_eq!(failure.error().class(), ErrorClass::Api);
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert!(api.code() == "InvalidResponse");
    assert!(!format!("{failure:?} {}", failure.error()).contains(BEARER));
}

#[path = "cost/tests.rs"]
mod cost;

#[path = "history/tests.rs"]
mod history;
