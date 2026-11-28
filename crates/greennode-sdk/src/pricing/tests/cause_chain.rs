use super::*;

#[test]
fn estimate_screens_and_drops_999_failures_on_a_small_stack() {
    const FAILURES: usize = 999;
    let server = Server::start(vec![response(OK, r#"{"optimumPrice":1e308}"#)]);
    let client = client(&server);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            runtime.block_on(async {
                let public_ips =
                    std::iter::once(EstimatePublicIp::new("quoted", "public-vip", "public", 1))
                        .chain((0..FAILURES).map(|index| {
                            EstimatePublicIp::new(
                                format!("failed-{index}"),
                                "public-vip",
                                "public",
                                2,
                            )
                        }))
                        .collect();
                let input = EstimateInput {
                    allow_partial: true,
                    public_ips,
                    ..EstimateInput::default()
                };
                let failure = client
                    .estimate(&RequestContext::new().0, input)
                    .await
                    .unwrap_err();
                let error = failure.error();
                assert_eq!(error.class(), ErrorClass::DiscoveryIncomplete);
                assert!(error.has_identity(ErrorClass::EstimateOverflow));
                assert!(!error.has_class(ErrorClass::Auth));
                assert!(error.api_error().is_none());
                assert!(error.login_error().is_none());
                assert!(error.service_account_login_error().is_none());
                assert!(error.detail::<crate::Diagnostic>().is_none());
                assert_eq!(
                    error.detail::<EstimateArithmeticError>().unwrap().item_key,
                    "failed-0"
                );
                assert_eq!(error.diagnostic_fallback(), "request failed");
                assert_eq!(error.to_string(), "greennode: DiscoveryIncomplete");
                assert_eq!(
                    format!("{error:?}"),
                    "Error { class: DiscoveryIncomplete, .. }"
                );
                let partial = failure.partial().unwrap();
                assert_eq!(partial.items.len(), FAILURES + 1);
                assert_eq!(partial.items[0].monthly_amount, Some(1e308));
                assert_eq!(partial.metadata.failures.len(), FAILURES);
                for (index, row) in partial.metadata.failures.iter().enumerate() {
                    assert_eq!(row.item_key, format!("failed-{index}"));
                    assert_eq!(row.code, "EstimateMultiplicationOverflow");
                }
                assert_eq!(server.requests(), 1);
                drop(failure);
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test]
async fn quote_screens_synthetic_status_against_operation_evidence() {
    let server = Server::start(vec![response(OK, "{}")]);
    let (ctx, _) = RequestContext::new();
    let mut evidence = crate::wire::product::evidence::Evidence::new();
    evidence.add_bearer(&Secret::new("200"));
    let failure = client(&server)
        .quote_with_evidence(&ctx, GetQuoteInput::new(RESOURCE_SERVER), &mut evidence)
        .await
        .unwrap_err();
    assert_eq!(failure.error().api_error().unwrap().status(), None);
}
