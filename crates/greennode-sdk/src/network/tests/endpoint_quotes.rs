use super::*;

#[tokio::test]
async fn endpoint_quote_requires_provenance_and_checks_catalog_placement() {
    let vserver = Server::start(vec![response(
        "200 OK",
        include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json"),
    )]);
    let vnetwork = Server::start(
        endpoint_quote_fixtures()
            .into_iter()
            .map(|body| response("200 OK", body))
            .collect(),
    );
    let config = Config::new(with_product_loopback(
        Product::Vnetwork,
        options(),
        &vnetwork,
    ))
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let input = QuoteCreateEndpointInput::new(
        "edge",
        "net-00000000-0000-0000-0000-000000000002",
        "sub-00000000-0000-0000-0000-000000000003",
    );
    let failure = Client::new(&config)
        .quote_create_endpoint(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    assert!(vnetwork.heads().is_empty() && vserver.heads().is_empty());
    let options = with_product_loopback(Product::Vserver, options(), &vserver);
    let options = with_product_loopback(Product::Vnetwork, options, &vnetwork);
    let config = Config::new(options).unwrap();
    let quote = Client::new(&config)
        .quote_create_endpoint(&ctx, input)
        .await
        .unwrap();
    assert_eq!(quote.monthly_price, 712_400.0);
    assert_eq!(quote.currency, "VND");
    assert!(vserver.heads()[0].starts_with(b"GET /v1/projects "));
    let heads = vnetwork.heads();
    assert_eq!(heads.len(), 8);
    assert!(heads[0].starts_with(b"GET /vnetwork/v1/regions "));
    assert!(heads[7].starts_with(b"POST /vnetwork/billing/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/price "));
}

fn endpoint_quote_fixtures() -> [&'static str; 8] {
    [
        include_str!("../../../tests/fixtures/network/endpoint_quote_regions.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_zones.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_categories.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_services.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_packages.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_vpcs.json"),
        include_str!("../../../tests/fixtures/network/endpoint_quote_subnets.json"),
        include_str!("../../../tests/fixtures/network/quote_create_nat.json"),
    ]
}
