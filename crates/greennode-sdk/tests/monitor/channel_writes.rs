use super::{response, target};
use greennode_sdk::monitor::{
    Client, CreateChannelInput, DeleteChannelInput, SendChannelOtpInput, UpdateChannelInput,
    WriteError,
};
use greennode_sdk::testing::{Product, Server, with_write_loopback};
use greennode_sdk::{Config, ConfigOptions, ErrorClass, RequestContext, Secret, WriteDispatch};

const CHANNELS: &str = include_str!("../fixtures/monitor/remaining_channels.json");
const CREATED: &str = r#"{"id":"channel-new","name":"Alerts","address":"example@example.test","createdDate":"2000-01-01"}"#;

fn client(server: &Server) -> Client {
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("toy-management"));
    Client::new(&Config::new(with_write_loopback(Product::Monitor, options, server)).unwrap())
}

#[tokio::test]
async fn create_channel_validates_otp_once_and_projects_the_authoritative_response() {
    let server = Server::start(vec![
        response("200 OK", r#"{"code":"operation-code"}"#),
        response("200 OK", CREATED),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateChannelInput::new("Alerts", "Email", "example@example.test");
    input.otp_ref = "ref-1".into();
    input.otp = Secret::new("123456");
    let output = client(&server).create_channel(&ctx, input).await.unwrap();
    assert_eq!(output.channel.id, "channel-new");
    let heads = server.heads();
    assert_eq!(
        target(&heads[0]).path(),
        "/notification-gateway/api/v1/notification/otps/validate"
    );
    assert_eq!(
        target(&heads[1]).path(),
        "/notification-gateway/api/v1/notification"
    );
    let bodies = server.bodies();
    let first: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(
        first
            == serde_json::json!({"ref":"ref-1","otp":"123456","address":"example@example.test","header":""})
    );
    assert!(
        second
            == serde_json::json!({"name":"Alerts","type":"Email","address":"example@example.test","header":"","otpCode":"operation-code"})
    );
}

#[tokio::test]
async fn rejected_otp_does_not_dispatch_the_channel_write() {
    let server = Server::start(vec![response("200 OK", r#"{"code":null}"#)]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateChannelInput::new("Alerts", "Email", "example@example.test");
    input.otp_ref = "ref-1".into();
    input.otp = Secret::new("123456");
    let error = client(&server)
        .create_channel(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<WriteError>(),
        Some(&WriteError::OtpRejected)
    );
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn send_channel_otp_does_not_retry_a_peer_error() {
    let server = Server::start(vec![
        response("503 Service Unavailable", "{}"),
        response("200 OK", r#"{"ref":"ref-1","expiredAt":946684800000}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .send_channel_otp(
            &ctx,
            SendChannelOtpInput::new("Email", "example@example.test"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(error.error().api_error().unwrap().status(), Some(503));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn update_channel_preserves_the_opaque_header_and_requires_guard_identity() {
    let current = CHANNELS.replace(
        r#"[{\"key\":\"X-Hook\",\"value\":\"toy-hook\"}]"#,
        "opaque-header",
    );
    let server = Server::start(vec![response("200 OK", &current), response("200 OK", "")]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdateChannelInput::new("channel-1");
    input.name = Some("Changed".into());
    let result = client(&server).update_channel(&ctx, input).await.unwrap();
    assert_eq!(result.channel.name, "Changed");
    assert_eq!(
        result.channel.for_display().address,
        "https://example.test/<redacted>"
    );
    let bodies = server.bodies();
    let body: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(body["header"] == "opaque-header");
}

#[tokio::test]
async fn delete_channel_maps_only_its_own_not_found_envelope() {
    let server = Server::start(vec![response(
        "400 Bad Request",
        r#"{"message":"Notification with id channel-1 is not found"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .delete_channel(&ctx, DeleteChannelInput::new("channel-1"))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotFound));
    assert_eq!(error.error().api_error().unwrap().status(), Some(400));
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 1);
}

fn transient(status: &str) -> greennode_sdk::testing::Response {
    greennode_sdk::testing::Response::Http {
        status: status.into(),
        headers: vec![
            ("Content-Length".into(), "0".into()),
            ("Retry-After".into(), "0".into()),
        ],
        body: Vec::new(),
    }
}

#[tokio::test]
async fn delete_and_plain_update_repeat_a_503_but_otp_update_sends_once() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        transient("503 Service Unavailable"),
        response("200 OK", ""),
    ]);
    client(&server)
        .delete_channel(&ctx, DeleteChannelInput::new("channel-1"))
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);
    let server = Server::start(vec![
        response("200 OK", CHANNELS),
        transient("503 Service Unavailable"),
        response("200 OK", ""),
    ]);
    let mut input = UpdateChannelInput::new("channel-1");
    input.name = Some("Changed".into());
    client(&server)
        .update_channel(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(server.requests(), 3);
    let channel = CHANNELS.replace("Webhook", "Email");
    let server = Server::start(vec![
        response("200 OK", &channel),
        response("200 OK", r#"{"code":"operation-code"}"#),
        transient("503 Service Unavailable"),
        response("200 OK", ""),
    ]);
    input.otp_ref = "ref-1".into();
    input.otp = Secret::new("123456");
    let error = client(&server)
        .update_channel(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn nonidempotent_otp_retries_a_429() {
    let server = Server::start(vec![
        transient("429 Too Many Requests"),
        response("200 OK", r#"{"ref":"ref-1","expiredAt":946684800000}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .send_channel_otp(
            &ctx,
            SendChannelOtpInput::new("Email", "example@example.test"),
        )
        .await
        .unwrap();
    assert_eq!(result.r#ref, "ref-1");
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn create_channel_requires_authoritative_id_and_update_refuses_guard_aliases() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response("200 OK", "")]);
    let error = client(&server)
        .create_channel(
            &ctx,
            CreateChannelInput::new("Alerts", "Webhook", "https://example.test/channel"),
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.message()),
        (Some(200), "create response had no id")
    );
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let body = CHANNELS.replace("\"typeNotification\"", "\"TypeNotification\"");
    let server = Server::start(vec![response("200 OK", &body)]);
    let mut input = UpdateChannelInput::new("channel-1");
    input.name = Some("Changed".into());
    let error = client(&server)
        .update_channel(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}
