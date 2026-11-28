use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Rotating(AtomicUsize);

impl crate::CredentialsProvider for Rotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            let number = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::Token::new(
                Secret::new(format!("toy-rotating-bearer-{number}")),
                None,
            ))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn all_rules_refuses_257th_distinct_bearer_before_request() {
    let groups: Vec<_> = (0..256)
        .map(|index| serde_json::json!({"id": format!("sg-{index}")}))
        .collect();
    let inventory = serde_json::json!({"listData":groups,"page":1,"pageSize":10000,"totalPage":1,"totalItem":256}).to_string();
    let mut replies = vec![reply("200 OK", &inventory)];
    replies.extend((0..256).map(|_| reply("200 OK", r#"{"data":[]}"#)));
    let server = Server::start(replies);
    let provider = Arc::new(Rotating(AtomicUsize::new(0)));
    let config = Config::new(with_product_loopback(
        Product::Vserver,
        ConfigOptions::default()
            .with_clock(Arc::new(crate::TestClock::new(
                time::OffsetDateTime::UNIX_EPOCH,
            )))
            .with_region("hcm-3".into())
            .with_project_id("proj-1".into())
            .with_credentials_provider(provider.clone()),
        &server,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotAWrite);
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 256);
    assert_eq!(provider.0.load(Ordering::SeqCst), 257);
}
