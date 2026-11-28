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

#[tokio::test]
async fn endpoint_quote_body_interruption_retains_context_at_discovery_and_price() {
    use crate::testing::ResponseBodyClock;
    use std::{sync::Arc, time::Duration};

    for (price, status, bearer) in [
        (false, 200, "toy-secret"),
        (true, 200, "toy-secret"),
        (false, 403, "toy-secret"),
        (true, 403, "toy-secret"),
        (false, 403, "403"),
        (false, 503, "503"),
        (false, 403, "canceled"),
        (false, 200, "canceled"),
    ] {
        let clock = Arc::new(ResponseBodyClock::new());
        let (head, _) = tokio::sync::oneshot::channel();
        let stalled = if status != 200 {
            let (eof, _) = tokio::sync::oneshot::channel();
            Response::ObserveStall {
                head: format!("HTTP/1.1 {status} Refused\r\nContent-Length: 10\r\n\r\n")
                    .into_bytes(),
                sent: head,
                eof,
            }
        } else {
            Response::Stall(head, 10)
        };
        let (projects, stages) = if price {
            let project =
                include_str!("../../../tests/fixtures/network/endpoint_quote_projects.json");
            (
                vec![response("200 OK", project)],
                endpoint_quote_fixtures()
                    .into_iter()
                    .take(7)
                    .map(|body| response("200 OK", body))
                    .chain([stalled])
                    .collect(),
            )
        } else {
            (vec![stalled], vec![])
        };
        let vserver = Server::start(projects);
        let vnetwork = Server::start(stages);
        let options = with_product_loopback(
            Product::Vserver,
            options()
                .with_static_token(Secret::new(bearer))
                .with_clock(clock.clone()),
            &vserver,
        );
        let config =
            Config::new(with_product_loopback(Product::Vnetwork, options, &vnetwork)).unwrap();
        let client = Client::new(&config);
        let (ctx, cancel) = RequestContext::new();
        let input = QuoteCreateEndpointInput::new(
            "edge",
            "net-00000000-0000-0000-0000-000000000002",
            "sub-00000000-0000-0000-0000-000000000003",
        );
        let interrupt = async {
            clock.wait_for_body(if price { 9 } else { 1 }).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(client.quote_create_endpoint(&ctx, input), interrupt)
        })
        .await
        .unwrap();
        let failure = result.unwrap_err();
        assert!(
            failure.error().has_identity(ErrorClass::Cancelled),
            "price stage: {price}"
        );
        if bearer == "canceled" {
            let error = failure.error();
            assert_eq!(vserver.requests(), 1);
            assert_eq!(vnetwork.requests(), 0);
            if status == 403 {
                assert!(error.has_identity(ErrorClass::PermissionDenied));
                let api = error.api_error().unwrap();
                assert_eq!((api.status(), api.code()), (Some(403), "Forbidden"));
                assert!(!format!("{api}").contains(bearer));
            } else {
                assert!(error.api_error().is_none());
            }
            assert!(!format!("{error} {failure:?}").contains(bearer));
            assert!(
                error
                    .joined_diagnostic()
                    .is_none_or(|message| !message.contains(bearer))
            );
            continue;
        }
        if status != 200 {
            let error = failure.error();
            if status == 403 {
                assert!(error.has_identity(ErrorClass::PermissionDenied));
            }
            let api = error.api_error().expect("observed refusal lost");
            let visible = (bearer == "toy-secret").then_some(status);
            let suffix = visible.map_or_else(String::new, |status| format!(" (status {status})"));
            assert_eq!(
                (api.status(), api.code()),
                (
                    visible,
                    if status == 403 {
                        "Forbidden"
                    } else {
                        "ServerError"
                    }
                )
            );
            assert_eq!(
                error.to_string(),
                format!(
                    "network.QuoteCreateEndpoint: endpoint response withheld{suffix}\nnetwork.QuoteCreateEndpoint: context canceled"
                )
            );
        } else {
            assert!(
                failure.error().api_error().is_none(),
                "price stage: {price}"
            );
            assert_eq!(
                failure.error().to_string(),
                "network.QuoteCreateEndpoint: context canceled"
            );
        }
    }
}
