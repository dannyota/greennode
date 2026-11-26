use super::*;

const CREATED: &[u8] = include_bytes!("../fixtures/CreateBudget.json");
const UPDATE_PATH: &[u8] = b"PUT /gateway/api/v1/budgets/budget-1 ";

fn create_input() -> CreateBudgetInput {
    CreateBudgetInput {
        name: "example-budget".into(),
        period_type: "MONTHLY".into(),
        r#type: "ACTUAL".into(),
        limit_amount: 1_000_000,
        status: String::new(),
    }
}

fn update_input() -> UpdateBudgetInput {
    UpdateBudgetInput {
        status: Some("PAUSED".into()),
        ..UpdateBudgetInput::new(BUDGET_ID)
    }
}

#[tokio::test]
async fn writes_send_baseline_requests_and_decode_success() {
    let server = Server::start(vec![
        response("201 Created", CREATED),
        response("200 OK", include_bytes!("../fixtures/UpdateBudget.json")),
        response("204 No Content", b""),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    let output = client.create_budget(&ctx, create_input()).await.unwrap();
    client
        .update_budget(
            &ctx,
            UpdateBudgetInput {
                limit_amount: Some(0),
                ..update_input()
            },
        )
        .await
        .unwrap();
    client
        .delete_budget(&ctx, DeleteBudgetInput::new(BUDGET_ID))
        .await
        .unwrap();

    assert_eq!(output.budget.uuid, BUDGET_ID);
    assert_eq!(output.budget.limit_amount, 1_000_000);
    assert_eq!(server.requests(), 3);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"POST /gateway/api/v1/budgets "));
    assert!(heads[1].starts_with(UPDATE_PATH));
    assert!(heads[2].starts_with(b"DELETE /gateway/api/v1/budgets/budget-1 "));
    let bodies = server.bodies();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bodies[0]).unwrap()
            == serde_json::json!({
                "name": "example-budget", "periodType": "MONTHLY", "type": "ACTUAL",
                "limitAmount": 1_000_000, "status": "ACTIVE",
            })
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bodies[1]).unwrap()
            == serde_json::json!({
                "limitAmount": 0, "status": "PAUSED",
            })
    );
    assert!(bodies[2].is_empty());
}

#[tokio::test]
async fn writes_never_replay_after_http_or_transport_failures() {
    let (ctx, _) = RequestContext::new();
    for (reply, status, advice) in [
        (Response::Close, None, true),
        (response("503 Service Unavailable", b""), Some(503), true),
        (
            Response::Http {
                status: "307 Temporary Redirect".into(),
                headers: vec![
                    ("Location".into(), "/other".into()),
                    ("Content-Length".into(), "0".into()),
                ],
                body: vec![],
            },
            Some(307),
            true,
        ),
        (response("401 Unauthorized", b""), Some(401), false),
    ] {
        let server = Server::start(vec![reply, response("200 OK", b"")]);
        let failure = client(&server, BEARER)
            .update_budget(&ctx, update_input())
            .await
            .unwrap_err();
        assert_eq!(server.requests(), 1);
        assert_eq!(server.connections(), 1);
        assert!(server.heads()[0].starts_with(UPDATE_PATH));
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), status);
        assert!(!api.retryable());
        assert_eq!(
            api.message().contains("read current state before retrying"),
            advice
        );
    }
}

#[tokio::test]
async fn malformed_and_reflected_successes_leave_outcome_unconfirmed() {
    let (ctx, _) = RequestContext::new();
    let reflected = format!(r#"{{"code":201,"data":{{"uuid":"budget-1","name":"{BEARER}"}}}}"#);
    for (status, body) in [
        ("201 Created", reflected.as_bytes()),
        (
            "201 Created",
            &br#"{"code":201,"data":{"name":"budget"}}"#[..],
        ),
        ("204 No Content", &b""[..]),
    ] {
        let server = Server::start(vec![response(status, body)]);
        let failure = client(&server, BEARER)
            .create_budget(&ctx, create_input())
            .await
            .unwrap_err();
        assert!(!format!("{failure:?} {}", failure.error()).contains(BEARER));
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.code(), "InvalidResponse");
        assert_eq!(
            api.status(),
            status
                .split_once(' ')
                .and_then(|(code, _)| code.parse().ok())
        );
        assert!(api.message().contains("read current state before retrying"));
        assert_eq!(server.requests(), 1);
    }
    let server = Server::start(vec![response("200 OK", reflected.as_bytes())]);
    let failure = client(&server, BEARER)
        .update_budget(&ctx, update_input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(!format!("{failure:?} {}", failure.error()).contains(BEARER));
}

#[tokio::test]
async fn local_validation_and_cancelled_context_are_not_sent() {
    let server = Server::start(vec![]);
    let (ctx, handle) = RequestContext::new();
    let client = client(&server, BEARER);
    let create = client
        .create_budget(
            &ctx,
            CreateBudgetInput {
                limit_amount: 0,
                ..create_input()
            },
        )
        .await
        .unwrap_err();
    let update = client
        .update_budget(&ctx, UpdateBudgetInput::new(BUDGET_ID))
        .await
        .unwrap_err();
    let delete = client
        .delete_budget(&ctx, DeleteBudgetInput::new("../x"))
        .await
        .unwrap_err();
    for (class, dispatch) in [
        (create.error().class(), create.write_dispatch()),
        (update.error().class(), update.write_dispatch()),
        (delete.error().class(), delete.write_dispatch()),
    ] {
        assert_eq!(class, ErrorClass::InvalidInput);
        assert_eq!(dispatch, WriteDispatch::NotSent);
    }
    handle.cancel();
    let failure = client
        .delete_budget(&ctx, DeleteBudgetInput::new(BUDGET_ID))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::Cancelled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn envelope_errors_keep_code_and_http_status_with_advice() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":500,"message":"boom","data":null}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, BEARER)
        .update_budget(&ctx, update_input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.code(), "500");
    assert_eq!(api.status(), Some(200));
    assert!(api.message().contains("read current state before retrying"));
}
