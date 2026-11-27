use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{
    CreateNotificationQuotaInput, DeleteNotificationQuotaInput, DeleteSyntheticQuotaInput,
    SetNotificationQuotaAutoRenewInput, SetSyntheticQuotaAutoRenewInput,
};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext, WriteDispatch};

const EMAIL: &str = include_str!("../fixtures/monitor/notification_quota_email.json");
const ABSENT: &str = include_str!("../fixtures/monitor/notification_quote_absent.json");
const PACKAGES: &str = include_str!("../fixtures/monitor/notification_quote_packages.json");
const SYNTH: &str = include_str!("../fixtures/monitor/synthetic_quote_renewal.json");
const DETAIL: &str = include_str!("../fixtures/monitor/quota_quota_detail.json");
const SYNTH_ABSENT: &str = include_str!("../fixtures/monitor/synthetic_quote_absent.json");
const FREE: &str = r#"{"optimumPrice":0}"#;

#[tokio::test]
async fn no_wait_notification_order_reconciles_without_resending_when_called_again() {
    let server = Server::start(vec![
        response("404 Not Found", ABSENT),
        response("200 OK", PACKAGES),
        response("200 OK", FREE),
        response("404 Not Found", ABSENT),
        response("200 OK", r#"{"orderId":"order-1","paymentUrl":null}"#),
        response("200 OK", EMAIL),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let mut input = CreateNotificationQuotaInput::new("Email");
    input.no_wait = true;
    let accepted = client
        .create_notification_quota(&ctx, input.clone())
        .await
        .unwrap();
    assert!(accepted.quota.is_none());
    assert_eq!(accepted.order_id, "order-1");
    let confirmed = client.create_notification_quota(&ctx, input).await.unwrap();
    assert_eq!(confirmed.quota.unwrap().id, "quota-email");
    assert_eq!(server.requests(), 6);
    assert!(server.heads()[4].starts_with(b"POST /billing-api/v1/email/quota "));
}

#[tokio::test]
async fn notification_delete_refuses_changed_terms_and_confirms_through_the_source_404() {
    let (ctx, _) = RequestContext::new();
    let changed = EMAIL.replace("emailfree", "emailplus");
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", &changed),
    ]);
    let mut input = DeleteNotificationQuotaInput::new("Email");
    input.expected_quota_id = "quota-email".into();
    let error = client(&server)
        .delete_notification_quota(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("204 No Content", ""),
        response("404 Not Found", ABSENT),
    ]);
    let output = client(&server)
        .delete_notification_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.stage, "settled");
    assert_eq!(output.quota.unwrap().id, "quota-email");
    assert!(server.heads()[2].starts_with(b"DELETE /billing-api/v1/email/quota "));
}

#[tokio::test]
async fn notification_disable_keeps_the_frozen_quota_in_settled_output() {
    let enabled = EMAIL.replace("\"autoRenew\": 0", "\"autoRenew\": 1");
    let disabled = EMAIL.replace("\"autoRenewPeriod\": 1", "\"autoRenewPeriod\": 0");
    let server = Server::start(vec![
        response("200 OK", &enabled),
        response("200 OK", &enabled),
        response("200 OK", ""),
        response("200 OK", &disabled),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .set_notification_quota_auto_renew(
            &ctx,
            SetNotificationQuotaAutoRenewInput::new("email", false),
        )
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(output.stage, "settled");
    assert_eq!(
        target(&server.heads()[2]).path(),
        "/billing-api/v1/email/quota/quota-email/update-auto-renew"
    );
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[2]).unwrap();
    assert!(body == serde_json::json!({"resourceId":"quota-email","enabled":false,"period":1}));
}

#[tokio::test]
async fn synthetic_auto_renew_enable_prices_and_freezes_the_free_quota() {
    let enabled = SYNTH.replace("\"autoRenew\": 0", "\"autoRenew\": 1");
    let server = Server::start(vec![
        response("200 OK", SYNTH),
        response("200 OK", DETAIL),
        response("200 OK", FREE),
        response("200 OK", SYNTH),
        response("200 OK", DETAIL),
        response("200 OK", ""),
        response("200 OK", &enabled),
        response("200 OK", DETAIL),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .set_synthetic_quota_auto_renew(&ctx, SetSyntheticQuotaAutoRenewInput::new(true))
        .await
        .unwrap();
    assert!(output.changed);
    assert_eq!(output.stage, "settled");
    assert!(server.heads()[2].starts_with(b"POST /billing-api/v2/synthetic/prices/created-price "));
    assert!(
        server.heads()[5]
            .starts_with(b"POST /billing-api/v1/synthetic/quota/quota-1/update-auto-renew ")
    );
}

#[tokio::test]
async fn synthetic_delete_requires_an_unexpired_snapshot_and_exact_absence() {
    let server = Server::start(vec![
        response("200 OK", SYNTH),
        response("200 OK", DETAIL),
        response("200 OK", SYNTH),
        response("200 OK", DETAIL),
        response("204 No Content", ""),
        response("404 Not Found", SYNTH_ABSENT),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .delete_synthetic_quota(&ctx, DeleteSyntheticQuotaInput::new("quota-1"))
        .await
        .unwrap();
    assert_eq!(output.stage, "settled");
    assert_eq!(output.quota.unwrap().id, "quota-1");
    assert!(server.heads()[4].starts_with(b"DELETE /billing-api/v1/synthetic/quota "));
}
