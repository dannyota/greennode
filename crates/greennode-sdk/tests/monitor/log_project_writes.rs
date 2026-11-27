use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{CreateLogProjectInput, DeleteLogProjectInput};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext, WriteDispatch};

const EMPTY: &str =
    r#"{"content":[],"currentPage":0,"pageSize":100,"totalPages":0,"totalElements":0}"#;
const CLASSES: &str = include_str!("../fixtures/monitor/remaining_log_classes.json");
const QUOTE: &str = include_str!("../fixtures/monitor/remaining_log_quote.json");

#[tokio::test]
async fn log_order_uses_the_exact_body_that_was_priced() {
    let server = Server::start(vec![
        response("200 OK", EMPTY),
        response("200 OK", CLASSES),
        response("200 OK", QUOTE),
        response("201 Created", r#"{"orderId":4001,"paymentUrl":"withheld"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogProjectInput::new("Logs");
    input.max_price = 5.0;
    input.no_wait = true;
    let output = client(&server)
        .create_log_project(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.order_id, "4001");
    assert!(output.log_project.id.is_empty());
    let bodies = server.bodies();
    assert!(bodies[2] == bodies[3]);
    assert_eq!(
        target(&server.heads()[2]).path(),
        "/billing-api/v2/log/prices/created-price"
    );
    assert_eq!(
        target(&server.heads()[3]).path(),
        "/billing-api/v2/log/quotas"
    );
}

#[tokio::test]
async fn log_order_above_price_sends_nothing_and_ambiguous_order_never_retries() {
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogProjectInput::new("Logs");
    input.no_wait = true;
    let server = Server::start(vec![
        response("200 OK", EMPTY),
        response("200 OK", CLASSES),
        response("200 OK", QUOTE),
    ]);
    let failure = client(&server)
        .create_log_project(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::PriceAboveMax);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
    input.max_price = 5.0;
    let server = Server::start(vec![
        response("200 OK", EMPTY),
        response("200 OK", CLASSES),
        response("200 OK", QUOTE),
        response("503 Service Unavailable", "{}"),
        response("201 Created", "{}"),
    ]);
    let failure = client(&server)
        .create_log_project(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn log_project_purge_after_a_missing_live_resource_uses_the_trash_route() {
    let server = Server::start(vec![
        response("404 Not Found", "{}"),
        response("204 No Content", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = DeleteLogProjectInput::new("project-1");
    input.purge = true;
    input.no_wait = true;
    client(&server)
        .delete_log_project(&ctx, input)
        .await
        .unwrap();
    let heads = server.heads();
    assert!(heads[0].starts_with(b"DELETE /billing-api/v1/log/quotas/project-1 "));
    assert!(heads[1].starts_with(b"DELETE /billing-api/v1/trash/log/quotas/project-1 "));
}

#[tokio::test]
async fn name_lookup_refuses_a_lone_nested_status_alias() {
    let row = include_str!("../fixtures/monitor/remaining_log_project.json")
        .replace("\"status\"", "\"Status\"");
    let page = format!(
        r#"{{"content":[{}],"currentPage":0,"pageSize":100,"totalPages":1,"totalElements":1}}"#,
        row
    );
    let server = Server::start(vec![response("200 OK", &page)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_log_project(&ctx, CreateLogProjectInput::new("Project"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().map(|error| error.code()),
        Some("InvalidResponse")
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn retained_project_is_screened_against_bearer_from_later_page() {
    use super::quota_writes::{DispatchBearer, with_dispatch_bearer};
    let page = |current, row| {
        format!(
            r#"{{"content":[{row}],"currentPage":{current},"pageSize":100,"totalPages":2,"totalElements":2}}"#
        )
    };
    let server = Server::start(vec![
        response("200 OK", EMPTY),
        response("200 OK", CLASSES),
        response("200 OK", QUOTE),
        response("201 Created", r#"{"orderId":4001,"paymentUrl":null}"#),
        response(
            "200 OK",
            &page(
                0,
                r#"{"id":"project-1","name":"Logs","status":"ACTIVE","description":"fixture-later-bearer"}"#,
            ),
        ),
        response(
            "200 OK",
            &page(
                1,
                r#"{"id":"project-2","name":"Another","status":"ACTIVE"}"#,
            ),
        ),
    ]);
    let source = std::sync::Arc::new(DispatchBearer::changing_at(5, "fixture-later-bearer"));
    let client = with_dispatch_bearer(&server, source);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogProjectInput::new("Logs");
    input.max_price = 5.0;
    let Err(failure) = client.create_log_project(&ctx, input).await else {
        panic!("retained project failed credential screening");
    };
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        failure.error().api_error().map(|error| error.code()),
        Some("InvalidResponse")
    );
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn failed_project_settlement_withholds_a_retained_encoded_id() {
    use super::quota_writes::{DispatchBearer, with_dispatch_bearer};
    let server = Server::start(vec![
        response("200 OK", EMPTY),
        response("200 OK", CLASSES),
        response("200 OK", QUOTE),
        response("201 Created", r#"{"orderId":4001,"paymentUrl":null}"#),
        response(
            "200 OK",
            r#"{"content":[{"id":"prefixYWJjsuffix","name":"Logs","status":"CREATING"}],"currentPage":0,"pageSize":100,"totalPages":1,"totalElements":1}"#,
        ),
        response("403 Forbidden", "{}"),
    ]);
    let client = with_dispatch_bearer(
        &server,
        std::sync::Arc::new(DispatchBearer::changing_at(5, "abc")),
    );
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogProjectInput::new("Logs");
    input.max_price = 5.0;
    let failure = client.create_log_project(&ctx, input).await.unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 6);
}
