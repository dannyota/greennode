use super::*;
use crate::pricing::{Client, GetQuoteInput, RESOURCE_SERVER};
use crate::wire::test_server::Server;

#[tokio::test]
async fn pricing_accepts_numeric_signin_code_and_refuses_its_reflection() {
    const CODE: &str = "248631";
    let page = "<input name=\"_csrf\" value=\"toy-csrf\">";
    let signin = Server::start(vec![
        response("200 OK", None, page),
        response("302 Found", Some("/ap/auth/iam/google"), ""),
        response("200 OK", None, page),
        response(
            "302 Found",
            Some("https://dashboard.test/return/?code=stub-code"),
            "",
        ),
    ]);
    let token = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"accessToken":"opaque-test-bearer","expiresIn":3600}"#,
    )]);
    let portal = Server::start(vec![
        response("200 OK", None, r#"{"optimumPrice":10}"#),
        response(
            "200 OK",
            None,
            &format!(r#"{{"optimumPrice":10,"propertiesPrice":[{{"name":"{CODE}"}}]}}"#),
        ),
    ]);
    fn code(
        _: &RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + '_>> {
        Box::pin(async { Ok(Secret::new(CODE)) })
    }
    let auth = iam().with_totp_provider(Arc::new(crate::TotpFn::new(code)));
    let mut config = Config::new(options().with_iam_user(auth).with_endpoint_overrides(
        EndpointOverrides {
            signin: Some(signin.origin.clone()),
            dashboard: Some("https://dashboard.test/return".into()),
            token: Some(format!("{}/token", token.origin)),
            portal: Some(portal.origin.clone()),
            ..Default::default()
        },
    ))
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::IamUser(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_roots(vec![signin.root_der.clone(), token.root_der.clone()]);
    session.set_test_product_root(portal.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    let output = client
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap();
    assert_eq!(output.optimum_price, 10.0);
    let error = client
        .get_quote(&ctx, GetQuoteInput::new(RESOURCE_SERVER))
        .await
        .unwrap_err();
    assert!(
        error
            .error()
            .api_error()
            .is_some_and(|api| api.code() == "InvalidResponse")
    );
    assert!(!format!("{error:?}").contains(CODE));
    assert_eq!(signin.requests(), 4);
    assert_eq!(portal.requests(), 2);
}
