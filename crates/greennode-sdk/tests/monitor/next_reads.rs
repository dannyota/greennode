use super::{client, client_with_token, response, target};
use greennode_sdk::monitor::{CreateNotificationQuotaInput, GetCheckAlarmInput};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext};

const ABSENT: &str = include_str!("../fixtures/monitor/notification_quote_absent.json");
const PACKAGES: &str = include_str!("../fixtures/monitor/notification_quote_packages.json");
const PRICE: &str = include_str!("../fixtures/monitor/notification_quote_paid.json");
const SNAPSHOT: &str = include_str!("../fixtures/monitor/check_alarm_snapshot.json");
const DEFINITION: &str = include_str!("../fixtures/monitor/check_alarm_definition.json");

#[tokio::test]
async fn notification_quote_preserves_paid_prices_and_only_posts_quote_fields() {
    let server = Server::start(vec![
        response("404 Not Found", ABSENT),
        response("200 OK", PACKAGES),
        response("200 OK", PRICE),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateNotificationQuotaInput::new("eMaIl");
    input.package_id = "emailplus".into();
    input.max_price = -1.0;
    input.no_wait = true;
    let output = client(&server)
        .quote_create_notification_quota(&ctx, input)
        .await
        .unwrap();
    let output = serde_json::to_value(output).unwrap();
    assert_eq!(output["ListPrice"], 100.0);
    assert_eq!(output["GuardPrice"], 120.0);
    let heads = server.heads();
    assert_eq!(target(&heads[0]).path(), "/billing-api/v1/email/quota");
    assert_eq!(target(&heads[1]).path(), "/billing-api/v1/email/packages");
    assert!(heads[2].starts_with(b"POST /billing-api/v2/email/prices/created-price "));
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[2]).unwrap();
    assert!(body == serde_json::json!({"redirectUrl":"", "packageId":"emailplus", "quantity":1}));
}

#[tokio::test]
async fn notification_quote_requires_exact_absence_and_unique_free_package() {
    let (ctx, _) = RequestContext::new();
    let input = CreateNotificationQuotaInput::new("Email");
    let server = Server::start(vec![response("404 Not Found", "{}")]);
    let error = client(&server)
        .quote_create_notification_quota(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    assert_eq!(server.requests(), 1);
    let ambiguous = PACKAGES.replace("\"price\": 100", "\"price\": 0");
    let server = Server::start(vec![
        response("404 Not Found", ABSENT),
        response("200 OK", &ambiguous),
    ]);
    let error = client(&server)
        .quote_create_notification_quota(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 2);
    let server = Server::start(vec![
        response("404 Not Found", ABSENT),
        response("200 OK", PACKAGES),
        response("200 OK", r#"{"optimumPrice":0}"#),
    ]);
    let output = client(&server)
        .quote_create_notification_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.package_id, "emailfree");
}

#[tokio::test]
async fn read_check_alarm_projects_the_current_definition_and_location() {
    let server = Server::start(vec![
        response("200 OK", SNAPSHOT),
        response("200 OK", DEFINITION),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_check_alarm(&ctx, GetCheckAlarmInput::new("chk-1"))
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["Alarm"]["Status"], "OK");
    assert_eq!(json["Alarm"]["Locations"][0]["LocationName"], "Probe");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vmonitor-uptime-manager/v1/uptimes/chk-1 "));
    assert!(
        heads[1].starts_with(b"GET /vmonitor-api/api/v1/alarms/synthetic/metrics/mona/alarm-1 ")
    );
}

#[tokio::test]
async fn check_alarm_refuses_missing_references_and_reflected_peer_ids() {
    let (ctx, _) = RequestContext::new();
    let input = GetCheckAlarmInput::new("chk-1");
    let no_reference = SNAPSHOT.replace("alarm-1", "");
    let server = Server::start(vec![response("200 OK", &no_reference)]);
    let failure = client(&server)
        .get_check_alarm(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotFound));
    assert_eq!(server.requests(), 1);
    let reflected = DEFINITION.replace("sub-1", "prefixYWJjsuffix");
    let server = Server::start(vec![
        response("200 OK", SNAPSHOT),
        response("200 OK", &reflected),
    ]);
    let failure = client_with_token(&server, "abc")
        .get_check_alarm(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn notification_quote_with_empty_kind_says_requires_kind_without_requests() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_create_notification_quota(&ctx, CreateNotificationQuotaInput::new(""))
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(
        error.error().detail::<String>().map(String::as_str),
        Some("vngcloud: invalid input: monitor.QuoteCreateNotificationQuota requires Kind")
    );
    assert_eq!(server.requests(), 0);
}
