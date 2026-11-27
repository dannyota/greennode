use super::channel_writes::client;
use super::{response, target};
use greennode_sdk::monitor::{
    CreateCheckInput, DeleteCheckInput, PauseCheckInput, ResumeCheckInput, UpdateCheckInput,
    WriteError,
};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext, WriteDispatch};

const CHECK: &str = include_str!("../fixtures/monitor/remaining_check.json");

#[tokio::test]
async fn check_create_uses_console_defaults_and_a_201_acknowledgement() {
    let server = Server::start(vec![response("201 Created", CHECK)]);
    let (ctx, _) = RequestContext::new();
    let input = CreateCheckInput::new("Probe", "https://example.test/", vec!["loc-1".into()]);
    let outcome = client(&server).create_check(&ctx, input).await.unwrap();
    assert_eq!(outcome.check.id, "chk-1");
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    assert!(body["config"]["request"]["method"] == "GET");
    assert!(body["config"]["request"]["timeout"] == 10);
    assert!(body["config"]["request"]["verified_ssl"] == true);
    assert!(body["options"]["failed_locations"] == 1);
    assert!(body["notifications"]["In-alarm"] == serde_json::json!([]));
    assert!(body.get("status").is_none());
    assert!(server.heads()[0].starts_with(b"POST /vmonitor-uptime-manager/v1/uptimes "));
}

#[tokio::test]
async fn check_update_merges_the_guard_snapshot_and_reads_once_on_empty_ack() {
    let after = CHECK.replace("\"Probe\"", "\"Changed\"");
    let server = Server::start(vec![
        response("200 OK", CHECK),
        response("200 OK", ""),
        response("200 OK", &after),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateCheckInput::new("chk-1");
    input.name = Some("Changed".into());
    let output = client(&server).update_check(&ctx, input).await.unwrap();
    assert_eq!(output.check.name, "Changed");
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(body["config"]["request"]["headers"]["X-Value"] == "one");
    assert!(body["notifications"]["In-alarm"][0] == "channel-1");
    assert!(body.get("status").is_none());
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn pause_confirms_after_a_503_and_keeps_a_preexisting_pause() {
    let disabled = CHECK.replace("ENABLED", "DISABLED");
    let server = Server::start(vec![
        response("200 OK", CHECK),
        response("503 Service Unavailable", "{}"),
        response("200 OK", &disabled),
        response("200 OK", &disabled),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let first = client
        .pause_check(&ctx, PauseCheckInput::new("chk-1"))
        .await
        .unwrap();
    assert!(first.changed);
    let second = client
        .pause_check(&ctx, PauseCheckInput::new("chk-1"))
        .await
        .unwrap();
    assert!(!second.changed);
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[1].starts_with(b"PUT /vmonitor-uptime-manager/v1/uptimes/status/chk-1 "));
}

#[tokio::test]
async fn resume_keeps_a_definitive_refusal_and_rejects_unknown_state_without_sending() {
    let (ctx, _) = RequestContext::new();
    let disabled = CHECK.replace("ENABLED", "DISABLED");
    let server = Server::start(vec![
        response("200 OK", &disabled),
        response("403 Forbidden", "{}"),
    ]);
    let error = client(&server)
        .resume_check(&ctx, ResumeCheckInput::new("chk-1"))
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::PermissionDenied);
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 2);
    let unknown = CHECK.replace("ENABLED", "OTHER");
    let server = Server::start(vec![response("200 OK", &unknown)]);
    let error = client(&server)
        .resume_check(&ctx, ResumeCheckInput::new("chk-1"))
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<WriteError>(),
        Some(&WriteError::UnexpectedStatus)
    );
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn delete_check_uses_its_own_route_and_accepts_a_204() {
    let server = Server::start(vec![response("204 No Content", "")]);
    let (ctx, _) = RequestContext::new();
    client(&server)
        .delete_check(&ctx, DeleteCheckInput::new("chk-1"))
        .await
        .unwrap();
    let head = &server.heads()[0];
    assert!(head.starts_with(b"DELETE "));
    assert_eq!(
        target(head).path(),
        "/vmonitor-uptime-manager/v1/uptimes/chk-1"
    );
}

#[tokio::test]
async fn check_privacy_limit_is_enforced_before_post() {
    let server = Server::start(vec![response("201 Created", CHECK)]);
    let (ctx, _) = RequestContext::new();
    let mut input = greennode_sdk::monitor::CreateCheckInput::new(
        "Probe",
        "https://example.test",
        vec!["loc-1".into()],
    );
    input.headers = (0..256)
        .map(|index| {
            (
                format!("x-fixture-{index}"),
                format!("fixture-private-{index}"),
            )
        })
        .collect();
    let failure = client(&server).create_check(&ctx, input).await.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn exhausted_check_query_scan_refuses_before_guard_read() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let mut input = greennode_sdk::monitor::UpdateCheckInput::new("chk-1");
    input.url = Some(format!(
        "https://example.test/?{}{}",
        "key=repeated&".repeat(270),
        (0..257)
            .map(|index| format!("key=separate-{index}&"))
            .collect::<String>()
    ));
    let failure = client(&server).update_check(&ctx, input).await.unwrap_err();
    let api = failure
        .error()
        .api_error()
        .expect("missing scoped privacy error");
    assert_eq!(api.code(), "InvalidResponse");
    assert_eq!(
        api.message(),
        "credential observation failed; response withheld"
    );
    assert_eq!(api.operation(), "monitor.UpdateCheck");
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn exhausted_probe_json_depth_refuses_the_write() {
    let server = Server::start(vec![response("201 Created", CHECK)]);
    let (ctx, _) = RequestContext::new();
    let mut input = greennode_sdk::monitor::CreateCheckInput::new(
        "Probe",
        "https://example.test",
        vec!["loc-1".into()],
    );
    input.body = format!("{}null{}", "[".repeat(129), "]".repeat(129));
    let Err(failure) = client(&server).create_check(&ctx, input).await else {
        panic!("the exhausted scan allowed the check write");
    };
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(server.heads().is_empty());
}

#[tokio::test]
async fn repeats_do_not_consume_the_distinct_privacy_budget() {
    let server = Server::start(vec![response("201 Created", CHECK)]);
    let (ctx, _) = RequestContext::new();
    let input = greennode_sdk::monitor::CreateCheckInput::new(
        "Probe",
        format!("https://example.test/?{}", "key=repeated&".repeat(270)),
        vec!["loc-1".into()],
    );
    assert!(client(&server).create_check(&ctx, input).await.is_ok());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn check_privacy_scans_the_first_json_value_before_trailing_data() {
    let reflected = CHECK.replace("Probe", "fixture-probe-secret");
    let server = Server::start(vec![response("201 Created", &reflected)]);
    let (ctx, _) = RequestContext::new();
    let mut input = greennode_sdk::monitor::CreateCheckInput::new(
        "Probe",
        "https://example.test",
        vec!["loc-1".into()],
    );
    input.body = r#"{"password":"fixture-probe-secret"} trailing text"#.into();
    let result = client(&server).create_check(&ctx, input).await;
    assert!(result.is_err());
    assert_eq!(server.requests(), 1);
}
