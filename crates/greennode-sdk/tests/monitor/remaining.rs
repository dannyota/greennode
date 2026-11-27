use super::{client, response, target};
use greennode_sdk::monitor::ListCheckAlarmHistoryInput;
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext};

const SNAPSHOT: &str = include_str!("../fixtures/monitor/check_alarm_snapshot.json");
const DEFINITION: &str = include_str!("../fixtures/monitor/check_alarm_definition.json");
const HISTORY: &str = include_str!("../fixtures/monitor/remaining_history.json");

fn history_input() -> ListCheckAlarmHistoryInput {
    let mut input = ListCheckAlarmHistoryInput::new("chk-1");
    input.start = "2000-01-01T07:00:00+07:00".into();
    input.end = "2000-01-05T00:00:00Z".into();
    input
}

#[tokio::test]
async fn alarm_history_reads_the_location_route_and_keeps_the_reported_window() {
    let server = Server::start(vec![
        response("200 OK", SNAPSHOT),
        response("200 OK", DEFINITION),
        response("200 OK", HISTORY),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_check_alarm_history(&ctx, history_input())
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Start"], "2000-01-01T07:00:00+07:00");
    assert_eq!(json["Items"][0]["SubAlarms"][0]["CurrentValues"][0], 2.5);
    let route = target(&server.heads()[2]);
    assert_eq!(
        route.path(),
        "/vmonitor-api/api/v1/alarms/synthetic/metrics/mona/sub-1/histories"
    );
    assert_eq!(
        route.query(),
        Some("end_time=947030400000&interval=5&start_time=946684800000")
    );
}

#[tokio::test]
async fn alarm_history_refuses_nonmatching_alarm_identity_without_partial_output() {
    let body = HISTORY.replace("sub-1", "sub-2");
    let server = Server::start(vec![
        response("200 OK", SNAPSHOT),
        response("200 OK", DEFINITION),
        response("200 OK", &body),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .list_check_alarm_history(&ctx, history_input())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    let server = Server::start(vec![]);
    let mut range = history_input();
    range.end = range.start.clone();
    let failure = client(&server)
        .list_check_alarm_history(&ctx, range)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn channel_lookup_walks_past_underreported_page_counts() {
    use greennode_sdk::monitor::GetChannelInput;
    let channel = include_str!("../fixtures/monitor/remaining_channels.json");
    let first = channel
        .replace("channel-1", "different")
        .replace("\"totalItem\": 1", "\"totalItem\": 2");
    let server = Server::start(vec![
        response("200 OK", &first),
        response("200 OK", channel),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .get_channel(&ctx, GetChannelInput::new("channel-1"))
        .await
        .unwrap();
    let shown = result.channel.for_display();
    assert_eq!(shown.address, "https://example.test/<redacted>");
    assert_eq!(shown.headers[0].value, "<redacted>");
    assert_eq!(
        target(&server.heads()[1])
            .query_pairs()
            .find(|(key, _)| key == "page")
            .unwrap()
            .1,
        "2"
    );
}

#[tokio::test]
async fn output_reads_ignore_lone_aliases_and_reject_nested_collisions() {
    use greennode_sdk::monitor::GetCheckInput;
    let check = include_str!("../fixtures/monitor/remaining_check.json");
    let alias = check.replace("\"test_frequency\"", "\"TEST_FREQUENCY\"");
    let collision = check.replace(
        "\"test_frequency\"",
        "\"TEST_FREQUENCY\": 1, \"test_frequency\"",
    );
    let server = Server::start(vec![
        response("200 OK", &alias),
        response("200 OK", &collision),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let result = client
        .get_check(&ctx, GetCheckInput::new("chk-1"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["Check"]["Options"]["TestFrequency"],
        0
    );
    let error = client
        .get_check(&ctx, GetCheckInput::new("chk-1"))
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
}

#[tokio::test]
async fn log_quote_distinguishes_missing_and_negative_prices() {
    use greennode_sdk::monitor::CreateLogProjectInput;
    let classes = include_str!("../fixtures/monitor/remaining_log_classes.json");
    let (ctx, _) = RequestContext::new();
    for (body, code, message) in [
        ("{\"optimumPrice\":null}", "", "quote response had no price"),
        (
            "{\"optimumPrice\":-1}",
            "InvalidResponse",
            "quote response had an invalid create price",
        ),
    ] {
        let server = Server::start(vec![response("200 OK", classes), response("200 OK", body)]);
        let error = client(&server)
            .quote_create_log_project(&ctx, CreateLogProjectInput::new("Logs"))
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(
            (api.status(), api.code(), api.message()),
            (Some(200), code, message)
        );
        let body: serde_json::Value = serde_json::from_slice(&server.bodies()[1]).unwrap();
        assert!(
            body == serde_json::json!({"redirectUrl":"https://vmonitor.console.vngcloud.vn/quota-usages/log","packageId":"retention-1","quantity":7,"buyWith":{},"monthPeriod":1,"projectName":"Logs","projectDescription":"","pay":true})
        );
    }
}

#[tokio::test]
async fn inventory_empty_body_defaults_only_for_ordinary_output_reads() {
    use greennode_sdk::monitor::{ListAlarmsInput, ListChecksInput};
    let server = Server::start(vec![response("200 OK", ""), response("200 OK", "")]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    assert!(
        client
            .list_checks(&ctx, ListChecksInput)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let failure = client
        .list_alarms(&ctx, ListAlarmsInput::new("Log"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), None);
}

#[tokio::test]
async fn log_quote_never_skips_an_unorderable_first_matching_class() {
    use greennode_sdk::monitor::CreateLogProjectInput;
    let classes = include_str!("../fixtures/monitor/remaining_log_classes.json");
    let first = serde_json::json!({"id":"other","name":"Basic","description":"Unavailable","priority":0,"status":"DISABLED"});
    let mut classes: Vec<serde_json::Value> = serde_json::from_str(classes).unwrap();
    classes.insert(0, first);
    let server = Server::start(vec![response(
        "200 OK",
        &serde_json::to_string(&classes).unwrap(),
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .quote_create_log_project(&ctx, CreateLogProjectInput::new("Logs"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 1);
}
