use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::monitor::CreateNotificationQuotaInput;
use crate::testing::{Product, Response, Server, with_write_loopback};
use crate::wire::product::evidence::SentCredentials;
use crate::{Config, ConfigOptions, CredentialsProvider, Secret, Token};

use super::*;

struct SwitchBearer(AtomicUsize, &'static str, usize);
impl CredentialsProvider for SwitchBearer {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            let index = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Token::new(
                Secret::new(if index == self.2 {
                    self.1
                } else {
                    "fixture-retained-0"
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
            AtomicUsize::new(0),
            "fixture-unretained",
            0,
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
    assert!(failure.partial().is_none());
    assert!(failure.error().api_error().is_none());
    {
        let pending = client.quota_ledger.pending.lock().unwrap();
        let retained = pending.get(&Kind::Email).unwrap();
        assert_eq!(retained.evidence.credentials().values().len(), 256);
        assert!(retained.order_id.is_empty());
    }
    let settled = client
        .create_notification_quota(&ctx, input)
        .await
        .unwrap_err();
    assert!(settled.partial().is_none());
    assert!(settled.error().api_error().is_none());
    assert!(client.quota_ledger.get(Kind::Email).unwrap().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn over_budget_bearer_cannot_echo_retained_status_or_fixed_output() {
    for first in ["200", "unconfirmed"] {
        let response = || Response::Http {
            status: "403 Forbidden".into(),
            headers: vec![("Content-Length".into(), "0".into())],
            body: Vec::new(),
        };
        let server = Server::start(vec![response(), response()]);
        let options = ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_credentials_provider(Arc::new(SwitchBearer(AtomicUsize::new(0), first, 0)));
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
            assert!(failure.partial().is_none());
            assert!(failure.error().api_error().is_none());
        }
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn quota_detail_lookup_refuses_after_an_observed_bearer_was_discarded() {
    const ACTIVE: &str =
        include_str!("../../../../../tests/fixtures/monitor/synthetic_quote_renewal.json");
    let active = ACTIVE.replace("2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let response = |status: &str, body: &str| Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    };
    let server = Server::start(vec![
        response("403 Forbidden", "{}"),
        response("200 OK", &active),
        response(
            "200 OK",
            include_str!("../../../../../tests/fixtures/monitor/quota_quota_detail.json"),
        ),
    ]);
    let client = pending_create(&server, "quota-1", 0);
    let (ctx, _) = RequestContext::new();
    let input = crate::monitor::CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    assert!(
        client
            .create_synthetic_quota(&ctx, input.clone())
            .await
            .is_err()
    );
    let next = client.create_synthetic_quota(&ctx, input).await;
    assert_eq!(server.requests(), 2);
    let Err(failure) = next else {
        panic!("the incomplete evidence allowed settlement")
    };
    assert!(failure.partial().is_none());
    assert!(failure.error().api_error().is_none());
}

fn pending_create(server: &Server, new_bearer: &'static str, index: usize) -> Client {
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(Arc::new(SwitchBearer(
            AtomicUsize::new(0),
            new_bearer,
            index,
        )));
    let config = Config::new(with_write_loopback(Product::Monitor, options, server)).unwrap();
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
    client
}

#[tokio::test]
async fn quota_detail_lookup_refuses_if_its_final_bearer_exceeds_the_limit() {
    const CURRENT: &str =
        include_str!("../../../../../tests/fixtures/monitor/synthetic_quote_renewal.json");
    let active = CURRENT.replace("2000-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let response = |body: &str| Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    };
    let server = Server::start(vec![
        response(&active),
        response(include_str!(
            "../../../../../tests/fixtures/monitor/quota_quota_detail.json"
        )),
    ]);
    let client = pending_create(&server, "fixture-new-detail-bearer", 1);
    let (ctx, _) = RequestContext::new();
    let input = crate::monitor::CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    let failure = client
        .create_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 1);
    assert!(failure.partial().is_none());
    assert!(failure.error().api_error().is_none());
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaEvidenceIncomplete)
    );
}
