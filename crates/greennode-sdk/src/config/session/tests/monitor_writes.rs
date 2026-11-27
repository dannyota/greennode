use super::*;

#[tokio::test]
async fn monitor_first_service_account_login_failure_keeps_the_create_preflight_envelope() {
    use crate::monitor::{Client, CreateSyntheticQuotaInput};
    use crate::wire::test_server::Server;

    let peer_text = "peer-diagnostic-canary";
    let login = Server::start(vec![response("403 Forbidden", None, peer_text)]);
    let monitor = Server::start(vec![]);
    let mut config = Config::new(
        options()
            .with_service_account(service())
            .with_product_loopback(crate::testing::Product::Monitor, &monitor, true),
    )
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::ServiceAccount(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_endpoint(
        format!("{}/accounts-api/v2/auth/token", login.origin),
        login.root_der.clone(),
    );

    let (ctx, _) = RequestContext::new();
    let failure = Client::new(&config)
        .create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
        )
        .await
        .unwrap_err();
    assert_eq!(login.requests(), 1);
    assert!(login.heads()[0].starts_with(b"POST /accounts-api/v2/auth/token "));
    assert_eq!(monitor.requests(), 0);
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);

    let partial = failure
        .partial()
        .expect("missing create preflight envelope");
    let output = serde_json::to_value(partial).unwrap();
    let error = failure.error();
    let visible = format!("{failure:?} {error} {output}");
    let authorization = data_encoding::BASE64.encode(b"client-canary:secret-canary");
    for fragment in [peer_text, "client-canary", "secret-canary", &authorization] {
        assert!(!visible.contains(fragment));
    }
    assert_eq!(output["Sent"], "no");
    assert_eq!(output["Stage"], "preflight");
    assert_eq!(output["Error"]["Code"], "ServiceAccountLoginFailed");
    assert_eq!(output["Error"]["Operation"], "monitor.CreateSyntheticQuota");
    assert_eq!(output["Error"]["Status"], 403);

    assert_eq!(error.class(), ErrorClass::Auth);
    let login_error = error.service_account_login_error().unwrap();
    assert_eq!(login_error.status(), Some(403));
    assert_eq!(login_error.reason(), "token endpoint rejected request");
}
