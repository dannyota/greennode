use super::*;

#[tokio::test]
async fn network_price_partials_keep_all_failures_and_screen_each_field() {
    for (bearer, forbidden, status) in [("403", "Forbidden", 0), ("Forbidden", "", 403)] {
        let quote = include_str!("../../../tests/fixtures/pricing/discovery_quote_public_vm.json");
        let retry = || Response::Http {
            status: "429 Too Many Requests".into(),
            headers: vec![
                ("Retry-After".into(), "1".into()),
                ("Content-Length".into(), "2".into()),
            ],
            body: b"{}".to_vec(),
        };
        let replies = [response("200 OK", quote), response("403 Forbidden", "{}")]
            .into_iter()
            .chain((0..4).map(|_| retry()))
            .collect();
        let server = Server::start(replies);
        let options = options()
            .with_project_id("proj-1".into())
            .with_static_token(Secret::new(bearer))
            .with_clock(crate::testing::retry_clock());
        let config = Config::new(with_product_loopback(Product::Portal, options, &server)).unwrap();
        let (ctx, _) = RequestContext::new();
        let failure = Client::new(&config)
            .list_network_prices(
                &ctx,
                ListNetworkPricesInput {
                    allow_partial: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(server.requests(), 6);
        let partial = failure
            .partial()
            .expect("expected usable price and failure rows");
        let rows = &partial.metadata.failures;
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].code.as_str(), rows[0].status), (forbidden, status));
        assert_eq!((rows[1].code.as_str(), rows[1].status), ("Throttled", 429));
        assert!(failure.error().has_identity(ErrorClass::PermissionDenied));
        assert!(failure.error().has_identity(ErrorClass::RateLimited));
    }
}
