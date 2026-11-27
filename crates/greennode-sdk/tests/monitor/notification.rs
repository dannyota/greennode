use super::{client, client_with_token, response, target};
use greennode_sdk::monitor::{GetNotificationQuotaInput, GetNotificationQuotaUsageInput};
use greennode_sdk::testing::{Response, Server};
use greennode_sdk::{ErrorClass, RequestContext};

const QUOTA: &str = include_str!("../fixtures/monitor/notification_quota_email.json");
const USAGE: &str = include_str!("../fixtures/monitor/notification_usage_email.json");

#[tokio::test]
async fn notification_quota_keeps_offset_and_sms_not_found_is_withheld() {
    let wire = QUOTA;
    let server = Server::start(vec![
        response("200 OK", wire),
        response("404 Not Found", "{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let output = client
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new("eMaIl"))
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Quota"]["ID"], "quota-email");
    assert_eq!(json["Quota"]["EndTime"], "2000-02-01T07:00:00+07:00");
    let failure = client
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new("sMs"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotFound);
    assert_eq!(
        failure.error().api_error().unwrap().message(),
        "quota response withheld"
    );
    let heads = server.heads();
    assert_eq!(target(&heads[0]).path(), "/billing-api/v1/email/quota");
    assert_eq!(target(&heads[1]).path(), "/billing-api/v1/sms/quota");
}

#[tokio::test]
async fn notification_quota_usage_has_typed_descriptions_and_refuses_sms_without_io() {
    let server = Server::start(vec![response("200 OK", USAGE)]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let error = client
        .get_notification_quota_usage(&ctx, GetNotificationQuotaUsageInput::new("SMS"))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(server.requests(), 0);
    let output = client
        .get_notification_quota_usage(&ctx, GetNotificationQuotaUsageInput::new("Email"))
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Usage"]["Amount"], 0);
    assert_eq!(json["Usage"]["Descriptions"]["amount"]["Name"], "Email");
    assert!(json["Usage"].get("Counts").is_none());
    assert_eq!(
        target(&server.heads()[0]).path(),
        "/billing-api/v1/email/quota/usages"
    );
}

#[tokio::test]
async fn notification_usage_screens_retained_text_and_map_keys() {
    let wire = USAGE
        .replace("\"name\": \"Email\"", "\"name\": \"toy%2Dtoken\"")
        .replace(
            "\"amount\": {",
            r#""toy%2Dtoken":{"name":"discarded","specs":"unused","unit":"unit"},"amount": {"#,
        );
    let server = Server::start(vec![response("200 OK", &wire)]);
    let (ctx, _) = RequestContext::new();
    let output = client_with_token(&server, "toy-token")
        .get_notification_quota_usage(&ctx, GetNotificationQuotaUsageInput::new("Email"))
        .await
        .unwrap();
    assert_eq!(output.usage.descriptions.len(), 1);
    assert_eq!(output.usage.descriptions["amount"].name, "<redacted>");
}

#[tokio::test]
async fn notification_quota_refuses_underscore_ids() {
    let wire = QUOTA.replace("quota-email", "quota_email");
    let server = Server::start(vec![response("200 OK", &wire)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new("email"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn notification_reads_with_empty_kind_say_requires_kind_before_any_request() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let client = client(&server);
    let message = |op: &str| format!("vngcloud: invalid input: {op} requires Kind");
    let error = client
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new(""))
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<String>(),
        Some(&message("monitor.GetNotificationQuota"))
    );
    let error = client
        .get_notification_quota_usage(&ctx, GetNotificationQuotaUsageInput::new(""))
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<String>(),
        Some(&message("monitor.GetNotificationQuotaUsage"))
    );
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn refused_redirect_on_a_redirect_following_read_has_no_status() {
    let server = Server::start(vec![Response::Http {
        status: "307 Temporary Redirect".into(),
        headers: vec![
            ("Location".into(), "https://other.example/x".into()),
            ("Content-Length".into(), "0".into()),
        ],
        body: Vec::new(),
    }]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new("email"))
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), None);
    assert_eq!(api.code(), "InvalidResponse");
}

#[tokio::test]
async fn empty_quota_body_says_invalid_structure_for_200_and_accepted_404() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response("200 OK", "")]);
    let failure = client(&server)
        .get_notification_quota(&ctx, GetNotificationQuotaInput::new("email"))
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.code(), "InvalidResponse");
    assert_eq!(api.message(), "quota response had invalid structure");

    let server = Server::start(vec![response("404 Not Found", "")]);
    let error = client(&server)
        .quote_create_notification_quota(
            &ctx,
            greennode_sdk::monitor::CreateNotificationQuotaInput::new("email"),
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(404));
    assert_eq!(api.message(), "quota response had invalid structure");
}
