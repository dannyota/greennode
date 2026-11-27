use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::monitor::CreateNotificationQuotaInput;
use crate::testing::{Product, Response, Server, with_write_loopback};
use crate::wire::product::evidence::SentCredentials;
use crate::{Config, ConfigOptions, CredentialsProvider, Secret, Token};

use super::*;

struct SwitchBearer(AtomicBool, &'static str);
impl CredentialsProvider for SwitchBearer {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            let was_used = self.0.swap(true, Ordering::SeqCst);
            Ok(Token::new(
                Secret::new(if was_used {
                    "fixture-retained-0"
                } else {
                    self.1
                }),
                None,
            ))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn quota_ledger_caps_credentials_and_settles_without_retained_order_data() {
    let body =
        include_bytes!("../../../../../tests/fixtures/monitor/notification_quota_email.json");
    let response = || Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    };
    let server = Server::start(vec![response(), response()]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(Arc::new(SwitchBearer(
            AtomicBool::new(false),
            "fixture-unretained",
        )));
    let config = Config::new(with_write_loopback(Product::Monitor, options, &server)).unwrap();
    let client = Client::new(&config);
    let mut evidence = Evidence::new();
    evidence.absorb(&SentCredentials::for_test(
        (0..256)
            .map(|index| Secret::new(format!("fixture-retained-{index}")))
            .collect(),
    ));
    let mut attempt = Attempt::new(
        "monitor.CreateNotificationQuota",
        Kind::Email,
        Goal::Notification {
            package: "emailfree".into(),
        },
        evidence,
    );
    attempt.order_id = "fixture-unretained".into();
    client.quota_ledger.put(&attempt).unwrap();
    let (ctx, _) = RequestContext::new();
    let input = CreateNotificationQuotaInput::new("Email");
    let failure = client
        .create_notification_quota(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    let partial = failure.partial().unwrap();
    assert!(partial.order_id.is_empty());
    assert!(partial.quota.is_none());
    {
        let pending = client.quota_ledger.pending.lock().unwrap();
        let retained = pending.get(&Kind::Email).unwrap();
        assert_eq!(retained.evidence.credentials().values().len(), 256);
        assert!(retained.order_id.is_empty());
    }
    let settled = client.create_notification_quota(&ctx, input).await.unwrap();
    assert!(settled.order_id.is_empty());
    assert!(settled.quota.is_none());
    assert!(client.quota_ledger.get(Kind::Email).unwrap().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn over_budget_bearer_cannot_echo_retained_status_or_fixed_output() {
    for (first, withhold) in [("200", false), ("unconfirmed", true)] {
        let response = || Response::Http {
            status: "403 Forbidden".into(),
            headers: vec![("Content-Length".into(), "0".into())],
            body: Vec::new(),
        };
        let server = Server::start(vec![response(), response()]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_credentials_provider(Arc::new(SwitchBearer(AtomicBool::new(false), first)));
        let config = Config::new(with_write_loopback(Product::Monitor, options, &server)).unwrap();
        let client = Client::new(&config);
        let mut evidence = Evidence::new();
        evidence.absorb(&SentCredentials::for_test(
            (0..256)
                .map(|index| Secret::new(format!("fixture-retained-{index}")))
                .collect(),
        ));
        let mut attempt = Attempt::new(
            "monitor.CreateSyntheticQuota",
            Kind::Synthetic,
            Goal::SyntheticCreate {
                class_id: "class-1".into(),
                package: "unit_1".into(),
                quantity: 10,
                location: 1,
                sent_at: QuotaTimestamp(OffsetDateTime::UNIX_EPOCH),
            },
            evidence,
        );
        attempt.observed_status = 200;
        client.quota_ledger.put(&attempt).unwrap();
        let (ctx, _) = RequestContext::new();
        let input = crate::monitor::CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
        for _ in 0..2 {
            let failure = client
                .create_synthetic_quota(&ctx, input.clone())
                .await
                .unwrap_err();
            assert!(failure.error().has_class(ErrorClass::NotSettled));
            if withhold {
                assert!(failure.partial().is_none());
            } else {
                let out = failure.partial().unwrap();
                assert_eq!(out.outcome.sent, "yes");
                assert!(out.outcome.error.as_ref().unwrap().status == ResponseField::Redacted);
            }
        }
        assert_eq!(server.requests(), 2);
    }
}
