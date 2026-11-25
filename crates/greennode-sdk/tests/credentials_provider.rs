use greennode_sdk::{CredentialsProvider, Error, RequestContext, Secret, Token};
use std::{future::Future, pin::Pin, sync::Arc};

struct Provider;

impl CredentialsProvider for Provider {
    fn token<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async { Ok(Token::new(Secret::new("toy-token"), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _rejected: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn provider_is_object_safe_and_callable() {
    let provider: Arc<dyn CredentialsProvider> = Arc::new(Provider);
    let (ctx, _) = RequestContext::new();
    let token = provider.token(&ctx).await.unwrap();
    assert!(token.access_token.expose_secret() == "toy-token");
    provider
        .invalidate(&ctx, &token.access_token)
        .await
        .unwrap();
}
