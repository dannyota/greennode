use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{client, response};
use crate::iam::{
    Client, CreateServiceAccountInput, issued_service_account_secret,
    redact_failed_service_account_create,
};
use crate::wire::test_server::Server;
use crate::{
    Config, ConfigOptions, CredentialsProvider, EndpointOverrides, Error, RequestContext, Secret,
    Token,
};

const CONFIRMATION_BEARER: &str = "toy-confirmation-bearer";

#[derive(Default)]
struct ChangingProvider(AtomicUsize);

impl CredentialsProvider for ChangingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let bearer = if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            "toy-create-bearer"
        } else {
            CONFIRMATION_BEARER
        };
        Box::pin(async move { Ok(Token::new(Secret::new(bearer), None)) })
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
async fn failed_confirmation_screens_private_file_path_with_fresh_bearer() {
    let server = Server::start(vec![
        response(
            "201 Created",
            br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
        ),
        response("404 Not Found", b"{}"),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_credentials_provider(Arc::new(ChangingProvider::default()))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 2);
    let path = format!("private/{CONFIRMATION_BEARER}/issued");
    assert!(redact_failed_service_account_create(&failure, &path) == "<redacted>");
    assert_eq!(
        redact_failed_service_account_create(&failure, "private/issued"),
        "private/issued"
    );
    assert!(
        issued_service_account_secret(&failure)
            .is_some_and(|secret| secret.expose_secret() == "toy-issued-key")
    );
    let partial = failure.partial().unwrap();
    assert!(partial.client_secret().is_none());
    assert!(!format!("{partial:?}").contains("toy-issued-key"));
}

#[tokio::test]
async fn cleanup_keeps_the_original_created_identity() {
    let server = Server::start(vec![
        response(
            "201 Created",
            br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
        ),
        response("200 OK", br#"{"id":"sa-1"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut output = client(&server, "toy-bearer")
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap();
    output.service_account.id = "sa-2".into();
    assert_eq!(output.cleanup_input().unwrap().service_account_id, "sa-1");
}
