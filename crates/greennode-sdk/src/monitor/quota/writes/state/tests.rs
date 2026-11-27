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

struct SwitchBearer(AtomicBool);
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
                    "fixture-unretained"
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
        .with_credentials_provider(Arc::new(SwitchBearer(AtomicBool::new(false))));
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
