use super::*;

const OVERVIEW: &[u8] = include_bytes!("../fixtures/GetCostOverview.json");
const GROUPED: &[u8] = include_bytes!("../fixtures/GetCostOverviewCaptured.json");
const RESOURCES: &str = include_str!("../fixtures/ListCostResources.json");
const BALANCES: &[u8] = include_bytes!("../fixtures/GetBalances.json");
const LEGACY_BALANCES: &[u8] = include_bytes!("../fixtures/GetBalancesUnenveloped.json");

const START: &str = "2026-09-01";
const END: &str = "2026-09-02";

#[tokio::test]
async fn overview_sends_defaults_and_filters_and_decodes_both_series_shapes() {
    let server = Server::start(vec![
        response("200 OK", OVERVIEW),
        response("200 OK", GROUPED),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let output = client
        .get_cost_overview(&ctx, GetCostOverviewInput::new(START, END))
        .await
        .unwrap();
    assert_eq!(output.summary.current_cost, 4_200_000.0);
    assert_eq!(output.summary.breakdown[0].cost, 2_500_000.0);
    assert!(output.grouped_series.is_none());
    assert_eq!(output.series.unwrap()[0].date, START);

    let mut input = GetCostOverviewInput::new(START, END);
    input.product = "compute".into();
    input.resource_type = "server".into();
    input.resource_id = "server-1".into();
    input.query = "web + api".into();
    input.group_by = "resourceType".into();
    input.interval = "weekly".into();
    let output = client.get_cost_overview(&ctx, input).await.unwrap();
    assert!(output.series.is_none());
    let groups = output.grouped_series.as_ref().unwrap();
    assert_eq!(groups[1].key, "vserver");
    assert_eq!(groups[1].color, "#11B895");
    assert_eq!(groups[1].points[0].date, "2026-11-01");
    assert_eq!(groups[1].points[0].cost, 20.0);

    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["GroupedSeries"][1]["Points"][0]["Cost"], 20.0);
    assert_eq!(json["Series"], serde_json::json!([]));

    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads[0].starts_with(b"GET /gateway/api/v2/cost-explorer/overview?endDate=2026-09-02&groupBy=product&interval=daily&startDate=2026-09-01 "));
    assert!(heads[1].starts_with(b"GET /gateway/api/v2/cost-explorer/overview?endDate=2026-09-02&groupBy=resourceType&interval=weekly&product=compute&q=web+%2B+api&resourceId=server-1&resourceType=server&startDate=2026-09-01 "));
}

#[tokio::test]
async fn overview_preserves_empty_series_and_prefers_present_breakdown_value() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":null,"data":{"summary":{"currentCost":0,"breakdown":[{"key":"compute","value":null,"cost":99,"percent":10}]},"series":[]}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .get_cost_overview(&ctx, GetCostOverviewInput::new(START, END))
        .await
        .unwrap();
    assert_eq!(output.series, Some(vec![]));
    assert_eq!(output.summary.breakdown[0].cost, 0.0);
}

#[tokio::test]
async fn resources_send_one_page_and_use_returned_metadata() {
    let page = RESOURCES.replace(r#""page": 0"#, r#""page": 1"#);
    let server = Server::start(vec![
        response("200 OK", RESOURCES.as_bytes()),
        response("200 OK", page.as_bytes()),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let mut input = ListCostResourcesInput::new(START, END);
    input.page = -3;
    input.size = 500;
    let output = client.list_cost_resources(&ctx, input).await.unwrap();
    assert_eq!(output.resources.page, 1);
    assert_eq!(output.summary.total_all, 340_000.0);

    let mut input = ListCostResourcesInput::new(START, END);
    input.page = 3;
    input.size = 50;
    input.sort = "cost".into();
    input.order = "desc".into();
    let output = client.list_cost_resources(&ctx, input).await.unwrap();
    assert_eq!(output.resources.page, 2);
    assert_eq!(output.resources.page_size, 200);
    assert_eq!(output.resources.total_page, 1);
    assert_eq!(output.resources.total_item, 2);
    assert_eq!(output.resources.items[0].resource_id, "server-1");
    assert_eq!(output.resources.items[0].cost, 250_000.0);
    assert_eq!(output.resources.items[0].daily_average, 125_000.5);

    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Items"][0]["ResourceID"], "server-1");
    assert_eq!(json["Summary"]["TotalAll"], 340_000.0);

    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads[0].starts_with(b"GET /gateway/api/v2/cost-explorer/resources?endDate=2026-09-02&page=0&size=200&startDate=2026-09-01 "));
    assert!(heads[1].starts_with(b"GET /gateway/api/v2/cost-explorer/resources?endDate=2026-09-02&order=desc&page=2&size=50&sort=cost&startDate=2026-09-01 "));
}

#[tokio::test]
async fn balances_accept_envelope_and_legacy_object_with_mixed_money_types() {
    let server = Server::start(vec![
        response("200 OK", BALANCES),
        response("200 OK", LEGACY_BALANCES),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let output = client
        .get_balances(&ctx, GetBalancesInput {})
        .await
        .unwrap();
    assert_eq!(output.balances.cash, Some(1_000_000.0));
    assert_eq!(output.balances.cash_available, Some(10_000_000.123_456_79));
    assert_eq!(output.balances.cash_holding, Some(50_000.0));
    assert_eq!(output.balances.poc, None);
    assert_eq!(output.balances.poc_holding, None);
    assert_eq!(
        client
            .get_balances(&ctx, GetBalancesInput {})
            .await
            .unwrap(),
        output
    );
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Balances"]["Cash"], 1_000_000.0);
    assert!(json["Balances"].get("POCHolding").unwrap().is_null());

    assert_eq!(server.requests(), 2);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET /navbar/balances/v1 "))
    );
}

#[tokio::test]
async fn balances_decode_fractional_numbers_and_go_numeric_strings() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"cash":"1_200.25","poc":"-0x_1.8p+2","cashAvailable":950000.25,"cashHolding":"0x1p-1074","pocHolding":"0x1p-1075"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .get_balances(&ctx, GetBalancesInput {})
        .await
        .unwrap();
    assert_eq!(output.balances.cash, Some(1_200.25));
    assert_eq!(output.balances.poc, Some(-6.0));
    assert_eq!(output.balances.cash_available, Some(950_000.25));
    assert_eq!(output.balances.cash_holding, Some(f64::from_bits(1)));
    assert_eq!(output.balances.poc_holding, Some(0.0));
}

#[tokio::test]
async fn overview_refuses_credentials_reflected_in_rendered_numbers() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"code":200,"data":{"summary":{"currentCost":0.0000001}}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let Err(failure) = client(&server, "e-7")
        .get_cost_overview(&ctx, GetCostOverviewInput::new(START, END))
        .await
    else {
        panic!("rendered credential reflection returned a cost overview");
    };
    assert_invalid_response(failure);
}

#[tokio::test]
async fn legacy_balances_refuse_a_numeric_bearer_absent_from_output() {
    let server = Server::start(vec![response("200 OK", LEGACY_BALANCES)]);
    let (ctx, _) = RequestContext::new();
    let Err(failure) = client(&server, "987654321")
        .get_balances(&ctx, GetBalancesInput {})
        .await
    else {
        panic!("numeric bearer returned legacy balances");
    };
    assert_invalid_response(failure);
}

#[tokio::test]
async fn invalid_dates_fail_without_requests() {
    let server = Server::start(vec![]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    for start in ["", "2026-02-29", "09-01-2026"] {
        let error = client
            .get_cost_overview(&ctx, GetCostOverviewInput::new(start, END))
            .await
            .unwrap_err();
        assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    }
    let error = client
        .list_cost_resources(&ctx, ListCostResourcesInput::new(START, "2026-13-01"))
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn invalid_cost_success_responses_withhold_the_body() {
    let bad_page = RESOURCES.replace(r#""page": 0"#, r#""page": 9223372036854775807"#);
    let missing_cost = RESOURCES.replace(r#""totalCost": 250000"#, r#""cost": 250000"#);
    let server = Server::start(vec![
        response("200 OK", br#"{"code":200,"data":{"summary":{"currentCost":0},"series":[{"date":"private-marker","cost":1,"points":null}]}}"#),
        response("200 OK", bad_page.as_bytes()),
        response("200 OK", missing_cost.as_bytes()),
        response("200 OK", br#"{"cash":"private-marker","poc":null,"cashAvailable":0,"cashHolding":null,"pocHolding":0}"#),
        response("200 OK", br#"{"cash":0,"poc":null,"cashAvailable":0,"cashHolding":null}"#),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    assert_invalid_response(
        client
            .get_cost_overview(&ctx, GetCostOverviewInput::new(START, END))
            .await
            .unwrap_err(),
    );
    for _ in 0..2 {
        assert_invalid_response(
            client
                .list_cost_resources(&ctx, ListCostResourcesInput::new(START, END))
                .await
                .unwrap_err(),
        );
    }
    for _ in 0..2 {
        assert_invalid_response(
            client
                .get_balances(&ctx, GetBalancesInput {})
                .await
                .unwrap_err(),
        );
    }
}

#[tokio::test]
async fn cost_reads_keep_http_and_envelope_errors() {
    let server = Server::start(vec![
        response(
            "400 Bad Request",
            br#"{"code":null,"message":"Invalid interval","data":null}"#,
        ),
        response(
            "200 OK",
            br#"{"code":400,"message":"Invalid resources","data":null}"#,
        ),
        response(
            "403 Forbidden",
            br#"{"code":403,"message":"Access denied","data":null}"#,
        ),
    ]);
    let client = client(&server, BEARER);
    let (ctx, _) = RequestContext::new();
    let error = client
        .get_cost_overview(&ctx, GetCostOverviewInput::new(START, END))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(400));
    assert_eq!(api.code(), "BadRequest");

    let error = client
        .list_cost_resources(&ctx, ListCostResourcesInput::new(START, END))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.code(), "400");

    let error = client
        .get_balances(&ctx, GetBalancesInput {})
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::PermissionDenied);
    assert_eq!(error.error().api_error().unwrap().status(), Some(403));
}

fn assert_invalid_response<T>(error: Failure<T>) {
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.code(), "InvalidResponse");
    assert!(!error.error().to_string().contains("private-marker"));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotAWrite);
}

#[tokio::test]
async fn truncated_cross_host_balance_redirect_keeps_no_status() {
    let redirect = || {
        Response::RawClose(
        b"HTTP/1.1 302 Found\r\nLocation: https://localhost:1/other\r\nContent-Length: 8\r\n\r\n{}"
            .to_vec(),
    )
    };
    let server = Server::start((0..4).map(|_| redirect()).collect());
    let (ctx, _) = RequestContext::new();
    let failure = client_with_options(
        &server,
        BEARER,
        ConfigOptions::default().with_clock(crate::testing::retry_clock()),
    )
    .get_balances(&ctx, GetBalancesInput {})
    .await
    .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!((api.status(), api.code(), api.message()), (None, "", ""));
    assert!(api.to_string().ends_with(": cross-host redirect refused"));
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn balance_decoder_screens_the_assembled_status_suffix() {
    let bearer = "403)";
    let server = Server::start(vec![response(
        "403 Forbidden",
        br#"{"message":"provider refused"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, bearer)
        .get_balances(&ctx, GetBalancesInput {})
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.operation(), api.status(), api.code()),
        ("billing.GetBalances", Some(403), "Forbidden")
    );
    assert!(!api.to_string().contains(bearer));
}
