use super::*;

#[tokio::test]
async fn monitor_quota_budget_scrubs_only_sent_bearers() {
    use crate::wire::test_server::Server;
    let padded = format!("{0}cid{0}", "<".repeat(350_000));
    let body = |text: &str| response("200 OK", None, text);
    let classes = include_str!("../../../../tests/fixtures/monitor/quota_classes.json")
        .replace("Coming soon.", &padded);
    let packages = include_str!("../../../../tests/fixtures/monitor/quota_v1_packages.json")
        .replace("free10", &padded);
    let iam = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"access_token":"token-canary","expires_in":120}"#,
    )]);
    let monitor = Server::start(vec![
        body(&classes),
        body(&packages),
        body("{}"),
        body("[]"),
    ]);
    let mut config = Config::new(
        options()
            .with_service_account(ServiceAccountAuth::new(
                "cid".into(),
                Secret::new("secret-canary"),
            ))
            .with_product_loopback(crate::testing::Product::Monitor, &monitor, false),
    )
    .unwrap();
    if let Auth::ServiceAccount(source) = &mut Arc::get_mut(&mut config.session).unwrap().auth {
        source.set_test_endpoint(iam.origin.clone(), iam.root_der.clone());
    } else {
        panic!("wrong auth kind");
    }
    let (ctx, _) = RequestContext::new();
    let failure = crate::monitor::Client::new(&config)
        .list_synthetic_packages(&ctx, crate::monitor::ListSyntheticPackagesInput)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "ResponseTooLarge"
    );
    assert_eq!(iam.requests(), 1);
}
