#![cfg(feature = "test-support")]

use greennode_sdk::monitor::{
    Client, GetDashboardInput, ListCheckResponseTimesInput, ListDashboardsInput,
};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{Config, ConfigOptions, ErrorClass, RequestContext, Secret};

const CHECK: &str =
    r#"{"id":"chk-1","subtype":"HTTP","history_alarm":"alarm-1","locations":["loc-1"]}"#;
const TIMES: &str = include_str!("fixtures/monitor/ListCheckResponseTimes.json");
const DASHBOARD: &str = include_str!("fixtures/monitor/get_dashboard_system.json");

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server) -> Client {
    client_with_token(server, "toy-monitor-token")
}

fn client_with_token(server: &Server, token: &str) -> Client {
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(token));
    Client::new(&Config::new(with_product_loopback(Product::Monitor, options, server)).unwrap())
}

fn target(head: &[u8]) -> url::Url {
    let target = std::str::from_utf8(head)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    url::Url::parse(&format!("https://example.test{target}")).unwrap()
}

#[tokio::test]
async fn dashboard_reads_use_console_routes_and_omit_widgets() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_str!("fixtures/monitor/list_dashboards_system.json"),
        ),
        response(
            "200 OK",
            include_str!("fixtures/monitor/list_dashboards_custom_empty.json"),
        ),
        response("200 OK", DASHBOARD),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let page = client
        .list_dashboards(
            &ctx,
            ListDashboardsInput {
                system: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items[0].id, "dash-1");
    assert!(
        client
            .list_dashboards(&ctx, ListDashboardsInput::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let detail = client
        .get_dashboard(&ctx, GetDashboardInput::new("dash-1"))
        .await
        .unwrap();
    let output = serde_json::to_value(detail).unwrap();
    assert_eq!(output["Dashboard"]["Name"], "Synthetic");
    assert!(output["Dashboard"].get("Widgets").is_none());
    let heads = server.heads();
    let list = target(&heads[0]);
    assert_eq!(list.path(), "/vmonitor-api/api/v1/dashboards");
    assert_eq!(
        list.query_pairs()
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>(),
        [
            ("filter", "systemType:productVng"),
            ("page", "1"),
            ("searching-field", ""),
            ("searching-text", ""),
            ("size", "10")
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .into()
    );
    assert_eq!(
        target(&heads[1])
            .query_pairs()
            .find(|(key, _)| key == "filter")
            .unwrap()
            .1,
        "system:false"
    );
    assert!(heads[2].starts_with(b"GET /vmonitor-api/api/v1/dashboards/dash-1 "));
}

#[tokio::test]
async fn dashboard_rejects_ambiguous_rows_and_withholds_service_errors() {
    let server = Server::start(vec![
        response("200 OK", r#"{"id":"dash-1","system":false,"System":true}"#),
        response("404 Not Found", r#"{"code":"summary","message":"echo"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let failure = client
        .get_dashboard(&ctx, GetDashboardInput::new("dash-1"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    let failure = client
        .get_dashboard(&ctx, GetDashboardInput::new("dash-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotFound);
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(404), "NotFound", "vMonitor response withheld")
    );
}

#[tokio::test]
async fn dashboard_refuses_a_configured_path_credential_before_sending() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let client = client_with_token(&server, "toy-canary");
    let failure = client
        .get_dashboard(&ctx, GetDashboardInput::new("toy-canary"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn response_times_pin_the_check_location_and_millisecond_window() {
    let server = Server::start(vec![response("200 OK", CHECK), response("200 OK", TIMES)]);
    let (ctx, _) = RequestContext::new();
    let mut input = ListCheckResponseTimesInput::new("chk-1");
    input.start = "2000-01-01t00:00:00.0009z".into();
    input.end = "2000-01-02T00:00:00.0009Z".into();
    let output = client(&server)
        .list_check_response_times(&ctx, input)
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Items"][0]["Points"][0]["ValueMillis"], 12.5);
    assert_eq!(json["Start"], "2000-01-01T00:00:00Z");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vmonitor-uptime-manager/v1/uptimes/chk-1 "));
    let metrics = target(&heads[1]);
    assert_eq!(metrics.path(), "/vmonitor-api/api/v1/statistics/synthetics");
    assert_eq!(
        metrics
            .query_pairs()
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>(),
        [
            ("alarm", "alarm-1"),
            ("dimensions", "syntt_id:chk-1"),
            ("end_time", "946771200000"),
            ("group_by", "location_id"),
            ("name", "syntt.http.response_time"),
            ("start_time", "946684800000"),
            ("statistics", "avg")
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .into()
    );
}

#[tokio::test]
async fn response_times_refuse_non_http_checks_before_requesting_statistics() {
    let snapshot = CHECK.replace("\"HTTP\"", "\"TCP\"");
    let server = Server::start(vec![response("200 OK", &snapshot)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .list_check_response_times(&ctx, ListCheckResponseTimesInput::new("chk-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn dashboard_redacts_reflected_booleans() {
    let server = Server::start(vec![response("200 OK", DASHBOARD)]);
    let (ctx, _) = RequestContext::new();
    let output = client_with_token(&server, "true")
        .get_dashboard(&ctx, GetDashboardInput::new("dash-1"))
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Dashboard"]["System"], "<redacted>");
    assert_eq!(json["Dashboard"]["Favorite"], "<redacted>");
}

#[tokio::test]
async fn response_times_redact_reflected_rendered_numbers() {
    let body = TIMES.replace("12.5", "12.50");
    let server = Server::start(vec![response("200 OK", CHECK), response("200 OK", &body)]);
    let (ctx, _) = RequestContext::new();
    let mut input = ListCheckResponseTimesInput::new("chk-1");
    input.end = "2000-01-02T00:00:00Z".into();
    let output = client_with_token(&server, "12.5")
        .list_check_response_times(&ctx, input)
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Items"][0]["Points"][0]["ValueMillis"], "<redacted>");
}
