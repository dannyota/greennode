use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{CreateLogAlarmInput, DeleteLogAlarmInput, UpdateLogAlarmInput};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext, WriteDispatch};

const PAGE: &str = r#"{"lstData":[],"page":1,"pageSize":10000,"totalItem":0,"totalPage":0}"#;
const PROJECT: &str = include_str!("../fixtures/monitor/remaining_log_project.json");
const ALARM: &str = include_str!("../fixtures/monitor/log_alarm_guard.json");

#[tokio::test]
async fn create_log_alarm_searches_exact_name_and_builds_console_defaults() {
    let server = Server::start(vec![
        response("200 OK", PAGE),
        response("200 OK", PROJECT),
        response("201 Created", r#"{"data":{"id":"alarm-1"}}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogAlarmInput::new("Read alarm", "project-1", 0.0);
    input.no_wait = true;
    let output = client(&server).create_log_alarm(&ctx, input).await.unwrap();
    assert_eq!(output.alarm_id, "alarm-1");
    assert!(output.alarm.id.is_empty());
    let heads = server.heads();
    assert_eq!(target(&heads[0]).path(), "/vmonitor-api/api/v1/alarms/list");
    assert!(heads[2].starts_with(b"POST /vmonitor-api/api/v1/alarms/logs "));
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[2]).unwrap();
    assert!(body["thresholdValue"] == 0.0);
    assert!(body["reason"] == "query(*) >0");
    assert!(body["projectName"] == "Project");
    assert!(body["resendPeriod"] == 30);
    assert!(body["groupByField"].is_null());
}

#[tokio::test]
async fn update_log_alarm_preserves_the_opaque_search_query_and_resends_lowercase_condition() {
    let server = Server::start(vec![response("200 OK", ALARM), response("200 OK", "")]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateLogAlarmInput::new("alarm-1");
    input.description = Some("Changed".into());
    input.no_wait = true;
    let output = client(&server).update_log_alarm(&ctx, input).await.unwrap();
    assert!(output.alarm.id.is_empty());
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(body["condition"] == "gt");
    assert!(body["logSearchQuery"] == "opaque-search");
    assert!(body["description"] == "Changed");
    assert!(body.get("resendEnabled").is_none());
    assert!(body["id"] == "log-1");
}

#[tokio::test]
async fn delete_log_alarm_only_deletes_a_guarded_log_alarm() {
    let (ctx, _) = RequestContext::new();
    let metric = ALARM.replace("LOG", "METRIC");
    let server = Server::start(vec![response("200 OK", &metric)]);
    let failure = client(&server)
        .delete_log_alarm(&ctx, DeleteLogAlarmInput::new("alarm-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
    let server = Server::start(vec![
        response("200 OK", ALARM),
        response("204 No Content", ""),
    ]);
    client(&server)
        .delete_log_alarm(&ctx, DeleteLogAlarmInput::new("alarm-1"))
        .await
        .unwrap();
    assert!(server.heads()[1].starts_with(b"DELETE /vmonitor-api/api/v1/alarms/logs/alarm-1 "));
}

#[tokio::test]
async fn name_search_never_writes_after_inconsistent_paging() {
    let inconsistent = PAGE.replace("\"totalPage\":0", "\"totalPage\":2");
    let server = Server::start(vec![response("200 OK", &inconsistent)]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogAlarmInput::new("Read alarm", "project-1", 0.0);
    input.no_wait = true;
    let failure = client(&server)
        .create_log_alarm(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn no_wait_alarm_create_tolerates_an_acknowledgement_without_identity_but_refuses_aliases() {
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogAlarmInput::new("Read alarm", "project-1", 0.0);
    input.no_wait = true;
    let server = Server::start(vec![
        response("200 OK", PAGE),
        response("200 OK", PROJECT),
        response("201 Created", r#""accepted""#),
    ]);
    let result = client(&server)
        .create_log_alarm(&ctx, input.clone())
        .await
        .unwrap();
    assert!(result.alarm_id.is_empty());
    let server = Server::start(vec![
        response("200 OK", PAGE),
        response("200 OK", PROJECT),
        response("201 Created", r#"{"data":{"ID":"alarm-1"}}"#),
    ]);
    let error = client(&server)
        .create_log_alarm(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn a_failed_alarm_status_preserves_the_observed_result_and_dispatch() {
    let failed = ALARM.replace("ACTIVE", "ERROR");
    let server = Server::start(vec![
        response("200 OK", PAGE),
        response("200 OK", PROJECT),
        response("201 Created", r#"{"id":"alarm-1"}"#),
        response("200 OK", &failed),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_log_alarm(
            &ctx,
            CreateLogAlarmInput::new("Read alarm", "project-1", 0.0),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::Failed);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let output = failure.partial().unwrap();
    assert_eq!(output.alarm_id, "alarm-1");
    assert_eq!(output.alarm.status, "ERROR");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn log_alarm_preserves_the_validated_raw_filter_object() {
    const FILTER: &str = r#"{"$serde_json::private::Number":"12"}"#;
    let server = Server::start(vec![
        response("200 OK", PAGE),
        response("200 OK", PROJECT),
        response("201 Created", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateLogAlarmInput::new("Read alarm", "project-1", 0.0);
    input.query_string = "search".into();
    input.filter = greennode_sdk::monitor::AlarmFilter(Some(
        serde_json::value::RawValue::from_string(FILTER.into()).unwrap(),
    ));
    input.no_wait = true;
    client(&server).create_log_alarm(&ctx, input).await.unwrap();
    let body: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&server.bodies()[2]).unwrap();
    assert!(body["filter"].get() == FILTER);
}

#[tokio::test]
async fn raw_log_search_query_is_excluded_from_debug() {
    let server = Server::start(vec![response("200 OK", ALARM)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_alarm(&ctx, greennode_sdk::monitor::GetAlarmInput::new("alarm-1"))
        .await
        .unwrap();
    assert!(!format!("{output:?}").contains("raw_log_search_query"));
}

#[tokio::test]
async fn failed_alarm_settlement_withholds_a_retained_raw_query() {
    use super::quota_writes::{DispatchBearer, with_dispatch_bearer};
    let mut pending: serde_json::Value = serde_json::from_str(ALARM).unwrap();
    pending["data"]["progressStatus"] = "CREATING".into();
    pending["data"]["alarmLog"]["logSearchQuery"] = "fixture-later-bearer".into();
    let server = Server::start(vec![
        response("200 OK", ALARM),
        response("200 OK", ""),
        response("200 OK", &pending.to_string()),
        response("403 Forbidden", "{}"),
    ]);
    let source = std::sync::Arc::new(DispatchBearer::changing_at(3, "fixture-later-bearer"));
    let client = with_dispatch_bearer(&server, source);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateLogAlarmInput::new("alarm-1");
    input.description = Some("Changed".into());
    let failure = client.update_log_alarm(&ctx, input).await.unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn unfinished_credential_observation_withholds_alarm_partial() {
    use super::quota_writes::{DispatchBearer, with_dispatch_bearer};
    let pending = ALARM.replace("ACTIVE", "CREATING");
    let server = Server::start(vec![
        response("200 OK", ALARM),
        response("200 OK", ""),
        response("200 OK", &pending),
    ]);
    let client = with_dispatch_bearer(
        &server,
        std::sync::Arc::new(DispatchBearer::changing_at(3, "")),
    );
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateLogAlarmInput::new("alarm-1");
    input.description = Some("Changed".into());
    let failure = client.update_log_alarm(&ctx, input).await.unwrap_err();
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 3);
}
