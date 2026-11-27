use super::{channel_writes::client, response, target};
use greennode_sdk::monitor::{
    CreateSyntheticQuotaInput, EnsureSyntheticQuotaInput, ErrorDetail, RecoverSyntheticQuotaInput,
    RenewSyntheticQuotaInput,
};
use greennode_sdk::testing::{Response, Server};
use greennode_sdk::{RequestContext, WriteDispatch};

const ABSENT: &str = include_str!("../fixtures/monitor/synthetic_quote_absent.json");
const ACTIVE: &str = include_str!("../fixtures/monitor/synthetic_quote_renewal.json");
const DETAIL: &str = include_str!("../fixtures/monitor/quota_quota_detail.json");
const CLASSES: &str = include_str!("../fixtures/monitor/quota_classes.json");
const UNITS: &str = include_str!("../fixtures/monitor/quota_class_packages.json");
const SETTINGS: &str = include_str!("../fixtures/monitor/quota_settings.json");
const FREE: &str = r#"{"optimumPrice":0}"#;
const ACK: &str = r#"{"orderId":"synth-order"}"#;

fn catalog(settings: &str) -> Vec<Response> {
    [CLASSES, UNITS, "[]", settings]
        .map(|body| response("200 OK", body))
        .into()
}
fn change_plan(before: &str, fresh: &str, settings: &str) -> Vec<Response> {
    let mut responses = vec![response("200 OK", before), response("200 OK", DETAIL)];
    responses.extend(catalog(SETTINGS));
    responses.push(response("200 OK", FREE));
    responses.extend(catalog(settings));
    responses.extend([response("200 OK", fresh), response("200 OK", DETAIL)]);
    responses
}

#[tokio::test]
async fn accepted_synthetic_create_is_reconciled_without_a_second_order() {
    let mut replies = vec![response("404 Not Found", ABSENT)];
    replies.extend(catalog(SETTINGS));
    replies.push(response("200 OK", FREE));
    replies.extend(catalog(SETTINGS));
    replies.extend([response("404 Not Found", ABSENT), response("200 OK", ACK)]);
    let created = ACTIVE.replace("2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    replies.extend([response("200 OK", &created), response("200 OK", DETAIL)]);
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let mut input = CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    input.no_wait = true;
    let first = client
        .create_synthetic_quota(&ctx, input.clone())
        .await
        .unwrap();
    assert_eq!(first.outcome.sent, "yes");
    assert_eq!(first.outcome.stage, "accepted");
    assert!(!first.created);
    let confirmed = client.create_synthetic_quota(&ctx, input).await.unwrap();
    assert_eq!(confirmed.outcome.stage, "confirmed");
    assert!(confirmed.created);
    assert_eq!(confirmed.quota.unwrap().id, "quota-1");
    assert_eq!(server.requests(), 14);
    assert!(server.heads()[11].starts_with(b"POST /billing-api/v2/synthetic/quota "));
}

#[tokio::test]
async fn early_renewal_returns_not_due_without_quoting_or_sending() {
    let mut replies = vec![response("200 OK", ACTIVE), response("200 OK", DETAIL)];
    replies.extend(catalog(SETTINGS));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.max_days_left = 7;
    let output = client(&server)
        .renew_synthetic_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.outcome.sent, "no");
    assert_eq!(output.outcome.stage, "not-due");
    assert!(output.previous_end_time.is_some());
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn recovery_requires_expired_trash_and_uses_the_trash_order_endpoint() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-", "2000-");
    let mut replies = change_plan(&trash, &trash, SETTINGS);
    replies.push(response("200 OK", ACK));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let mut input = RecoverSyntheticQuotaInput::new("quota-1");
    input.no_wait = true;
    let output = client(&server)
        .recover_synthetic_quota(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.outcome.stage, "accepted");
    assert!(!output.recovered);
    assert_eq!(
        target(&server.heads()[13]).path(),
        "/billing-api/v2/trash/synthetic/quota"
    );
}

#[tokio::test]
async fn ensure_refuses_when_the_final_catalog_changes_after_the_quote() {
    let settings = SETTINGS.replace("PRE-PAID", "POST-PAID");
    let mut replies = change_plan(ACTIVE, ACTIVE, &settings);
    replies.push(response("200 OK", ACK));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let input = EnsureSyntheticQuotaInput {
        expected_quota_id: "quota-1".into(),
        no_wait: true,
        ..Default::default()
    };
    let failure = client(&server)
        .ensure_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let result = failure.partial().unwrap();
    assert_eq!(result.action, "renew");
    assert_eq!(result.outcome.stage, "preflight");
    assert_eq!(server.requests(), 13);
}

#[tokio::test]
async fn final_quota_replacement_prevents_renewal() {
    let fresh = ACTIVE.replace("quota-1", "quota-2");
    let mut replies = change_plan(ACTIVE, &fresh, SETTINGS);
    let fresh_detail = DETAIL.replace("quota-1", "quota-2");
    *replies.last_mut().unwrap() = response("200 OK", &fresh_detail);
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(
            &ctx,
            RenewSyntheticQuotaInput::new("2099-02-01T0:00:00,000Z"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 13);
}

#[tokio::test]
async fn create_requires_the_exact_source_absence_response() {
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"message":"not found"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.partial().unwrap().outcome.stage, "preflight");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn pending_payment_preserves_send_evidence_until_a_different_intent_reconciles() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    replies.extend([
        response(
            "200 OK",
            r#"{"orderId":"synth-order","paymentUrl":"https://billing.example.test/pay"}"#,
        ),
        response("200 OK", &extended),
        response("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.no_wait = true;
    let failure = client.renew_synthetic_quota(&ctx, input).await.unwrap_err();
    let result = failure.partial().unwrap();
    assert!(result.payment_pending);
    assert_eq!(result.outcome.sent, "yes");
    assert_eq!(result.outcome.stage, "unconfirmed");
    let (expired, _) = RequestContext::new();
    let expired = expired.child_with_deadline(tokio::time::Instant::now());
    let input = EnsureSyntheticQuotaInput {
        expected_quota_id: "quota-1".into(),
        ..Default::default()
    };
    let failure = client
        .ensure_synthetic_quota(&expired, input.clone())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::DeadlineExceeded)
    );
    assert_eq!(failure.partial().unwrap().outcome.sent, "yes");
    assert_eq!(server.requests(), 14);
    let failure = client
        .ensure_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(failure.partial().unwrap().outcome.stage, "preflight");
    assert_eq!(server.requests(), 16);
}

#[tokio::test]
async fn recovery_refuses_a_zero_expiry() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-02-01T07:00:00+07:00", "0001-01-01T00:00:00Z");
    let server = Server::start(vec![response("200 OK", &trash), response("200 OK", DETAIL)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .recover_synthetic_quota(&ctx, RecoverSyntheticQuotaInput::new("quota-1"))
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::InvalidInput)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn reflected_order_id_does_not_erase_pending_payment() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response(
        "200 OK",
        r#"{"orderId":"toy-management","paymentUrl":"https://billing.example.test/pay"}"#,
    ));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let partial = failure.partial().unwrap();
    assert!(partial.payment_pending);
    assert_eq!(partial.order_id, "<redacted>");
}

#[tokio::test]
async fn oversized_order_keeps_the_observed_status() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response("200 OK", &"x".repeat((1 << 20) + 1)));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let outcome = &failure.partial().unwrap().outcome;
    assert_eq!(
        failure.error().api_error().map(|api| api.code()),
        Some("ResponseTooLarge")
    );
    assert_eq!(outcome.sent, "yes");
    assert_eq!(
        outcome.error.as_ref().unwrap().status,
        greennode_sdk::monitor::ResponseField::Value(200)
    );
}

#[tokio::test]
async fn different_auto_renew_guard_reconciles_pending_order_before_refusing() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    replies.extend([
        response("200 OK", ACK),
        response("200 OK", &extended),
        response("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut renew = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    renew.no_wait = true;
    client.renew_synthetic_quota(&ctx, renew).await.unwrap();
    let mut auto_renew = greennode_sdk::monitor::SetSyntheticQuotaAutoRenewInput::new(true);
    auto_renew.expected_quota_id = "quota-other".into();
    let failure = client
        .set_synthetic_quota_auto_renew(&ctx, auto_renew)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(server.requests(), 16);
}

#[tokio::test]
async fn cross_operation_reconciliation_keeps_the_callers_operation() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.extend([
        response("200 OK", ACK),
        response("403 Forbidden", r#"{"message":"ignored"}"#),
    ]);
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let mut renew = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    renew.no_wait = true;
    client.renew_synthetic_quota(&ctx, renew).await.unwrap();
    let failure = client
        .ensure_synthetic_quota(&ctx, EnsureSyntheticQuotaInput::default())
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.operation(), "monitor.EnsureSyntheticQuota");
    assert_eq!(api.status(), Some(403));
    assert_eq!(server.requests(), 15);
}

#[tokio::test]
async fn mismatched_synthetic_delete_reconciles_and_preserves_invalid_usage() {
    for settled in [true, false] {
        let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
        let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
        replies.push(response("200 OK", ACK));
        if settled {
            replies.extend([response("200 OK", &extended), response("200 OK", DETAIL)]);
        } else {
            replies.push(response("403 Forbidden", "{}"));
        }
        let server = Server::start(replies);
        let client = client(&server);
        let (ctx, _) = RequestContext::new();
        let mut renew = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
        renew.no_wait = true;
        client.renew_synthetic_quota(&ctx, renew).await.unwrap();
        let failure = client
            .delete_synthetic_quota(
                &ctx,
                greennode_sdk::monitor::DeleteSyntheticQuotaInput::new("different-quota"),
            )
            .await
            .unwrap_err();
        assert!(
            failure
                .error()
                .has_class(greennode_sdk::ErrorClass::InvalidInput)
        );
        assert_eq!(
            failure
                .error()
                .has_class(greennode_sdk::ErrorClass::NotSettled),
            !settled
        );
        assert_eq!(server.requests(), if settled { 16 } else { 15 });
    }
}

#[tokio::test]
async fn dropped_order_preserves_the_response_head_in_the_ledger() {
    use greennode_sdk::{Clock, SystemClock};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(Default)]
    struct OrderClock {
        remaining: AtomicUsize,
        head_observed: tokio::sync::Notify,
    }
    impl Clock for OrderClock {
        fn utc_now(&self) -> time::OffsetDateTime {
            SystemClock.utc_now()
        }
        fn monotonic_now(&self) -> tokio::time::Instant {
            SystemClock.monotonic_now()
        }
        fn sleep_until(
            &self,
            deadline: tokio::time::Instant,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            if self.remaining.load(Ordering::SeqCst) > 0
                && self.remaining.fetch_sub(1, Ordering::SeqCst) == 1
            {
                self.head_observed.notify_one();
            }
            SystemClock.sleep_until(deadline)
        }
    }
    struct Provider {
        clock: Arc<OrderClock>,
        calls: AtomicUsize,
    }
    impl greennode_sdk::CredentialsProvider for Provider {
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
                if self.calls.fetch_add(1, Ordering::SeqCst) == 13 {
                    self.clock.remaining.store(3, Ordering::SeqCst);
                }
                Ok(greennode_sdk::Token::new(
                    greennode_sdk::Secret::new("fixture-dispatch-bearer"),
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
    let (sent_head, _receipt) = tokio::sync::oneshot::channel();
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(Response::Stall(sent_head, 20));
    let server = Server::start(replies);
    let clock = Arc::new(OrderClock::default());
    let options = greennode_sdk::ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(Arc::new(Provider {
            clock: clock.clone(),
            calls: AtomicUsize::new(0),
        }));
    let options = greennode_sdk::testing::with_fixture_clock(options, clock.clone());
    let options = greennode_sdk::testing::with_write_loopback(
        greennode_sdk::testing::Product::Monitor,
        options,
        &server,
    );
    let client = greennode_sdk::monitor::Client::new(&greennode_sdk::Config::new(options).unwrap());
    let (ctx, _) = RequestContext::new();
    let input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    {
        let mut renew = std::pin::pin!(client.renew_synthetic_quota(&ctx, input.clone()));
        tokio::select! {
            () = clock.head_observed.notified() => (),
            _ = &mut renew => panic!("the order completed before the body was stalled"),
        }
    }
    let ctx = ctx.child_with_deadline(tokio::time::Instant::now());
    let failure = client.renew_synthetic_quota(&ctx, input).await.unwrap_err();
    let outcome = &failure.partial().unwrap().outcome;
    assert_eq!(outcome.sent, "yes");
    assert_eq!(
        outcome.error.as_ref().unwrap().status,
        greennode_sdk::monitor::ResponseField::Value(200)
    );
    assert_eq!(server.requests(), 14);
}

#[tokio::test]
async fn synthetic_create_uses_the_retained_auto_renew_settlement_bound() {
    let (started, settling) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("200 OK", FREE),
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("503 Service Unavailable", "{}"),
        Response::StallBeforeHead(started),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let first = client
        .set_synthetic_quota_auto_renew(
            &ctx,
            greennode_sdk::monitor::SetSyntheticQuotaAutoRenewInput::new(true),
        )
        .await
        .unwrap_err();
    assert!(
        first
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );

    let mut create = std::pin::pin!(client.create_synthetic_quota(
        &ctx,
        CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
    ));
    tokio::select! {
        biased;
        started = settling => started.unwrap(),
        _ = &mut create => panic!("reconciliation completed before the fixture held its response"),
    }
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let result = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(create.as_mut(), cx))
    })
    .await;
    let std::task::Poll::Ready(Err(failure)) = result else {
        panic!("creation did not honor the retained 30-second settlement bound");
    };
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::DeadlineExceeded)
    );
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn dropped_settlement_retains_the_later_bearer() {
    use super::quota_writes::{DispatchBearer, with_dispatch_bearer};
    let (started, receipt) = tokio::sync::oneshot::channel();
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    replies.extend([
        response("200 OK", ACK),
        Response::StallBeforeHead(started),
        response("200 OK", &extended),
        response("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let client = with_dispatch_bearer(
        &server,
        std::sync::Arc::new(DispatchBearer::changing_at(14, "synth-order")),
    );
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.no_wait = true;
    client
        .renew_synthetic_quota(&ctx, input.clone())
        .await
        .unwrap();
    {
        let mut settle = std::pin::pin!(client.renew_synthetic_quota(&ctx, input.clone()));
        tokio::select! {
            arrived = receipt => arrived.unwrap(),
            _ = &mut settle => panic!("the settlement finished before its response was held"),
        }
    }
    let output = client.renew_synthetic_quota(&ctx, input).await.unwrap();
    assert!(output.renewed);
    assert!(output.order_id == "<redacted>");
    assert_eq!(server.requests(), 17);
}

#[tokio::test]
async fn early_settlement_failure_exposes_no_previous_order_or_payment() {
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response(
        "200 OK",
        r#"{"orderId":"synth-order","paymentUrl":"https://example.test/pay"}"#,
    ));
    let server = Server::start(replies);
    let client = client(&server);
    let (ctx, cancel) = RequestContext::new();
    let input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    assert!(
        client
            .renew_synthetic_quota(&ctx, input.clone())
            .await
            .is_err()
    );
    cancel.cancel();
    let failure = client.renew_synthetic_quota(&ctx, input).await.unwrap_err();
    let output = failure.partial().unwrap();
    assert_eq!(output.outcome.sent, "yes");
    assert!(output.order_id.is_empty());
    assert!(!output.payment_pending);
    assert!(output.quota.is_none());
    assert_eq!(server.requests(), 14);
}

#[tokio::test]
async fn cancelled_delete_keeps_stale_guard_cause_without_snapshot() {
    let server = Server::start(vec![
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("200 OK", ACTIVE),
        response("200 OK", DETAIL),
        response("503 Service Unavailable", "{}"),
    ]);
    let client = client(&server);
    let (ctx, cancel) = RequestContext::new();
    let input = greennode_sdk::monitor::DeleteSyntheticQuotaInput::new("quota-1");
    assert!(client.delete_synthetic_quota(&ctx, input).await.is_err());
    cancel.cancel();
    let failure = client
        .delete_synthetic_quota(
            &ctx,
            greennode_sdk::monitor::DeleteSyntheticQuotaInput::new("other-quota"),
        )
        .await
        .unwrap_err();
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::NotSettled)
    );
    assert!(
        failure
            .error()
            .has_class(greennode_sdk::ErrorClass::InvalidInput)
    );
    let result = failure.partial().unwrap();
    assert_eq!(result.stage, "sent");
    assert!(result.quota.is_none());
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn renewal_checks_the_current_clock_after_catalog_reads() {
    use greennode_sdk::{Clock, SystemClock};
    use std::sync::Arc;
    struct CatalogClock(Arc<Server>);
    impl Clock for CatalogClock {
        fn utc_now(&self) -> time::OffsetDateTime {
            let day = if self.0.requests() < 6 { 22 } else { 27 };
            time::Date::from_calendar_date(2099, time::Month::January, day)
                .unwrap()
                .midnight()
                .assume_utc()
        }
        fn monotonic_now(&self) -> tokio::time::Instant {
            SystemClock.monotonic_now()
        }
        fn sleep_until(
            &self,
            deadline: tokio::time::Instant,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            SystemClock.sleep_until(deadline)
        }
    }
    let mut replies = change_plan(ACTIVE, ACTIVE, SETTINGS);
    replies.push(response("200 OK", ACK));
    let server = Arc::new(Server::start(replies));
    let options = greennode_sdk::ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(greennode_sdk::Secret::new("fixture-clock-bearer"));
    let options =
        greennode_sdk::testing::with_fixture_clock(options, Arc::new(CatalogClock(server.clone())));
    let options = greennode_sdk::testing::with_write_loopback(
        greennode_sdk::testing::Product::Monitor,
        options,
        &server,
    );
    let client = greennode_sdk::monitor::Client::new(&greennode_sdk::Config::new(options).unwrap());
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.max_days_left = 7;
    input.no_wait = true;
    let output = client.renew_synthetic_quota(&ctx, input).await.unwrap();
    assert_eq!(output.outcome.stage, "accepted");
    assert_eq!(server.requests(), 14);
}

#[path = "synthetic_writes/lifecycle_retained.rs"]
mod lifecycle_retained;
