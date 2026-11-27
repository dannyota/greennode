use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{
    CreateNotificationQuotaInput, DeleteNotificationQuotaInput, DeleteSyntheticQuotaInput,
    SetNotificationQuotaAutoRenewInput, SetSyntheticQuotaAutoRenewInput,
};
use greennode_sdk::testing::{Response, Server};
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

#[tokio::test]
async fn delete_never_uses_a_status_that_reflects_a_bearer() {
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("204 No Content", ""),
        response("404 Not Found", ABSENT),
    ]);
    let options = greennode_sdk::ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(greennode_sdk::Secret::new("ACTIVE"));
    let options = greennode_sdk::testing::with_write_loopback(
        greennode_sdk::testing::Product::Monitor,
        options,
        &server,
    );
    let config = greennode_sdk::Config::new(options).unwrap();
    let client = greennode_sdk::monitor::Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_notification_quota(
            &ctx,
            greennode_sdk::monitor::DeleteNotificationQuotaInput::new("Email"),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

pub(super) struct DispatchBearer {
    calls: std::sync::atomic::AtomicUsize,
    at: usize,
    third: &'static str,
    wait: Option<tokio::sync::Notify>,
}

impl DispatchBearer {
    pub(super) fn changing_at(at: usize, token: &'static str) -> Self {
        Self {
            calls: Default::default(),
            at,
            third: token,
            wait: None,
        }
    }
}

impl greennode_sdk::CredentialsProvider for DispatchBearer {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<greennode_sdk::Token, greennode_sdk::Error>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let third = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == self.at;
            if third && let Some(wait) = &self.wait {
                wait.notify_one();
                std::future::pending::<()>().await;
            }
            if third && self.third.is_empty() {
                return Err(greennode_sdk::Error::provider_failed());
            }
            Ok(greennode_sdk::Token::new(
                greennode_sdk::Secret::new(if third {
                    self.third
                } else {
                    "fixture-first-bearer"
                }),
                None,
            ))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a greennode_sdk::Secret,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), greennode_sdk::Error>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }
}

pub(super) fn with_dispatch_bearer(
    server: &Server,
    provider: std::sync::Arc<DispatchBearer>,
) -> greennode_sdk::monitor::Client {
    let options = greennode_sdk::ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(provider);
    let options = greennode_sdk::testing::with_write_loopback(
        greennode_sdk::testing::Product::Monitor,
        options,
        server,
    );
    greennode_sdk::monitor::Client::new(&greennode_sdk::Config::new(options).unwrap())
}

#[tokio::test]
async fn bodyless_delete_checks_identity_against_the_final_bearer() {
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("204 No Content", ""),
    ]);
    let client = with_dispatch_bearer(
        &server,
        std::sync::Arc::new(DispatchBearer {
            calls: Default::default(),
            at: 2,
            third: "quota-email",
            wait: None,
        }),
    );
    let (ctx, _) = RequestContext::new();
    let failure = client
        .delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn dropping_while_acquiring_final_bearer_leaves_no_pending_attempt() {
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("204 No Content", ""),
        response("404 Not Found", ABSENT),
    ]);
    let source = std::sync::Arc::new(DispatchBearer {
        calls: Default::default(),
        at: 2,
        third: "fixture-unused",
        wait: Some(tokio::sync::Notify::new()),
    });
    let client = with_dispatch_bearer(&server, source.clone());
    let (ctx, _) = RequestContext::new();
    {
        let mut attempt = std::pin::pin!(
            client.delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        );
        tokio::select! {
            () = source.wait.as_ref().unwrap().notified() => (),
            _ = &mut attempt => panic!("quota write completed before reaching final credential acquisition"),
        }
    }
    assert_eq!(server.requests(), 2);
    let result = client
        .delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        .await
        .unwrap();
    assert_eq!(result.stage, "settled");
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn dropping_a_dispatched_delete_preserves_the_pending_attempt() {
    let (received, receipt) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        Response::StallBeforeHead(received),
        response("404 Not Found", ABSENT),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    {
        let mut attempt = std::pin::pin!(
            client.delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        );
        tokio::select! {
            result = receipt => result.unwrap(),
            _ = &mut attempt => panic!("quota write completed before the fixture held its response"),
        }
    }
    let result = client
        .delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        .await
        .unwrap();
    assert_eq!(result.stage, "settled");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn notification_delete_retains_a_truncated_four_xx_attempt() {
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        Response::RawClose(b"HTTP/1.1 409 Conflict\r\nContent-Length: 20\r\n\r\n{".to_vec()),
        response("404 Not Found", ABSENT),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let input = DeleteNotificationQuotaInput::new("Email");
    let failure = client
        .delete_notification_quota(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    let result = client.delete_notification_quota(&ctx, input).await.unwrap();
    assert_eq!(result.stage, "settled");
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn completed_quota_throttle_preserves_its_retry_hint() {
    let server = Server::start(vec![
        response("200 OK", EMAIL),
        response("200 OK", EMAIL),
        response("429 Too Many Requests", r#"{"message":"server throttle"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .delete_notification_quota(&ctx, DeleteNotificationQuotaInput::new("Email"))
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(429));
    assert!(api.retryable());
    assert_eq!(server.requests(), 3);
}
