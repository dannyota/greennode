use super::*;
use crate::network::{
    Client, ListNatInstancesInput, ListNatZonesInput, QuoteCreateNatInstanceInput,
};
use crate::testing::{Product, Server};

const ZONE_PATH_DIAGNOSTIC: &str =
    "vngcloud: invalid input: network.ListNATZones requires ZoneID to match ^[A-Za-z0-9-]+$";

fn assert_nat_path_diagnostics(catalog: &Error, quote: &Error) {
    for (error, diagnostic) in [
        (catalog, ZONE_PATH_DIAGNOSTIC),
        (
            quote,
            "vngcloud: invalid input: network.QuoteCreateNATInstance requires PackageID to match ^[A-Za-z0-9-]+$",
        ),
    ] {
        assert_eq!(error.class(), ErrorClass::InvalidInput);
        assert!(error.api_error().is_none());
        assert!(error.detail::<String>().map(String::as_str) == Some(diagnostic));
        assert!(error.to_string() == diagnostic);
    }
}

#[tokio::test]
async fn nat_path_diagnostics_screen_a_bearer_from_a_prior_login() {
    let iam = Server::start(vec![
        response(
            "200 OK",
            None,
            r#"{"access_token":"token-canary","expires_in":120}"#,
        ),
        response(
            "200 OK",
            None,
            r#"{"access_token":"renewed-credential","expires_in":120}"#,
        ),
    ]);
    let inventory = || {
        response(
            "200 OK",
            None,
            r#"{"success":true,"page":1,"size":10,"totalPage":0,"total":0}"#,
        )
    };
    let network = Server::start(vec![inventory(), inventory()]);
    let clock = Arc::new(crate::TestClock::new(crate::SystemClock.utc_now()));
    let mut config = Config::new(
        options()
            .with_clock(clock.clone())
            .with_service_account(service())
            .with_project_id("proj-1".into())
            .with_product_loopback(Product::Vnetwork, &network, false),
    )
    .unwrap();
    let Auth::ServiceAccount(source) = &mut Arc::get_mut(&mut config.session).unwrap().auth else {
        panic!("expected service-account auth");
    };
    source.set_test_endpoint(iam.origin.clone(), iam.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    for requests in 1..=2 {
        client
            .list_nat_instances(
                &ctx,
                ListNatInstancesInput {
                    zone_id: "zone-1".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(iam.requests(), requests);
        assert_eq!(network.requests(), requests);
        clock.advance(Duration::from_secs(121));
    }

    let invalid = "bad/token-canary";
    let catalog = client
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: invalid.into(),
            },
        )
        .await
        .unwrap_err();
    let quote = client
        .quote_create_nat_instance(
            &ctx,
            QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", invalid, "vpc-1"),
        )
        .await
        .unwrap_err();
    assert_nat_path_diagnostics(catalog.error(), quote.error());
    let unreflected = client
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: "bad/path".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(unreflected.error().to_string() == ZONE_PATH_DIAGNOSTIC);
    assert_eq!(iam.requests(), 2);
    assert_eq!(network.requests(), 2);
}

#[tokio::test]
async fn nat_path_diagnostics_withhold_a_bearer_used_by_another_config() {
    let provider = Arc::new(Provider(AtomicUsize::new(0)));
    let network = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"success":true,"page":1,"size":10,"totalPage":0,"total":0}"#,
    )]);
    let options = options()
        .with_credentials_provider(provider.clone())
        .with_project_id("proj-1".into())
        .with_product_loopback(Product::Vnetwork, &network, false);
    let first = Config::new(options.clone()).unwrap();
    let second = Config::new(options).unwrap();
    assert!(!Arc::ptr_eq(&first.session, &second.session));
    let (ctx, _) = RequestContext::new();
    Client::new(&first)
        .list_nat_instances(
            &ctx,
            ListNatInstancesInput {
                zone_id: "zone-1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let client = Client::new(&second);
    let invalid = "bad/token-canary";
    let catalog = client
        .list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: invalid.into(),
            },
        )
        .await
        .unwrap_err();
    let quote = client
        .quote_create_nat_instance(
            &ctx,
            QuoteCreateNatInstanceInput::new("edge", "zone-1", "az-1", invalid, "vpc-1"),
        )
        .await
        .unwrap_err();
    assert_nat_path_diagnostics(catalog.error(), quote.error());
    assert_eq!(provider.0.load(Ordering::Relaxed), 1);
    assert_eq!(network.requests(), 1);
}

#[tokio::test]
async fn nat_path_diagnostics_omit_invalid_values_after_incomplete_authentication() {
    struct FailingProvider(AtomicUsize);

    impl CredentialsProvider for FailingProvider {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Err(Error::new(ErrorClass::Auth)) })
        }

        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    let provider = Arc::new(FailingProvider(AtomicUsize::new(0)));
    let server = Server::start(Vec::new());
    let config = Config::new(
        options()
            .with_credentials_provider(provider.clone())
            .with_product_loopback(Product::Vnetwork, &server, false),
    )
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let client = Client::new(&config);
    let validate = || {
        client.list_nat_zones(
            &ctx,
            ListNatZonesInput {
                zone_id: "bad/path".into(),
            },
        )
    };
    let before = validate().await.unwrap_err();
    assert!(before.error().to_string() == ZONE_PATH_DIAGNOSTIC);
    assert_eq!(provider.0.load(Ordering::Relaxed), 0);
    assert!(config.authenticate(&ctx).await.is_err());
    let failure = validate().await.unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
    assert!(failure.error().api_error().is_none());
    assert!(failure.error().to_string() == ZONE_PATH_DIAGNOSTIC);
    assert_eq!(provider.0.load(Ordering::Relaxed), 1);
    assert_eq!(server.requests(), 0);
}
