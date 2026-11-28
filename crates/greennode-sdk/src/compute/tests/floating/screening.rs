use super::*;

#[tokio::test]
async fn floating_create_refuses_reflected_acknowledgement() {
    for (token, acknowledgement) in [
        ("jkl", r#"{"data":{"uuid":"prefix6A6b6Csuffix"}}"#),
        (
            "toy-token",
            r#"{"data":{"uuid":"server-1","extra":"dG95LXRva2Vu"}}"#,
        ),
    ] {
        let server = TestServer::start(vec![
            response(LIST),
            response(GROUP),
            response(SAFE),
            response(QUOTE),
            response(GROUP),
            response(SAFE),
            http("202 Accepted", acknowledgement),
        ]);
        let (ctx, _) = RequestContext::new();
        let options = ConfigOptions::default().with_static_token(Secret::new(token));
        let failure = floating_client(&server, options)
            .create_server(&ctx, floating_input())
            .await
            .unwrap_err();
        assert_eq!(
            failure.write_dispatch(),
            crate::WriteDispatch::MayHaveBeenSent
        );
        assert!(failure.partial().is_none());
        assert_eq!(server.heads().len(), 7);
    }
}

#[tokio::test]
async fn regression_partial_price_screens_fixed_decimal_encoding() {
    struct Late(AtomicUsize);
    impl CredentialsProvider for Late {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            Box::pin(async move {
                Ok(Token::new(
                    Secret::new(if self.0.fetch_add(1, Ordering::SeqCst) < 7 {
                        CANARY
                    } else {
                        "12"
                    }),
                    None,
                ))
            })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let quote = r#"{"optimumPrice":0.00003132,"originalPrice":0.00003132,"discountPrice":0,"discountPercent":0,"propertiesPrice":[]}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(quote),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(r#"{"data":{"uuid":"server-1","status":"ERROR"}}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let options =
        ConfigOptions::default().with_credentials_provider(Arc::new(Late(AtomicUsize::new(0))));
    let failure = floating_client(&server, options)
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
}

#[tokio::test]
async fn regression_guard_rejects_top_level_encoded_identity() {
    let inventory = r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":0,"projectId":"prefix6A6b6Csuffix"}"#;
    let server = TestServer::start(vec![response(inventory), response(GROUP), response(SAFE)]);
    let (ctx, _) = RequestContext::new();
    let options = ConfigOptions::default().with_static_token(Secret::new("jkl"));
    let failure = floating_client(&server, options)
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn regression_confirmation_rejects_top_level_encoded_identity() {
    let confirmation =
        r#"{"projectId":"prefix6A6b6Csuffix","data":{"uuid":"server-1","status":"ACTIVE"}}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        http("202 Accepted", ACK),
        response(confirmation),
    ]);
    let (ctx, _) = RequestContext::new();
    let options = ConfigOptions::default().with_static_token(Secret::new("jkl"));
    let failure = floating_client(&server, options)
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(server.heads().len(), 8);
}

#[tokio::test]
async fn regression_local_inventory_errors_screen_credentials() {
    for (token, inventory) in [
        (
            "incomplete",
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":2,"totalItem":1}"#,
        ),
        (
            "already",
            r#"{"listData":[{"uuid":"server-1","name":"example"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
        ),
    ] {
        let server = TestServer::start(vec![response(inventory)]);
        let (ctx, _) = RequestContext::new();
        let options = ConfigOptions::default().with_static_token(Secret::new(token));
        let failure = floating_client(&server, options)
            .create_server(&ctx, floating_input())
            .await
            .unwrap_err();
        assert!(
            !failure
                .error()
                .api_error()
                .unwrap()
                .message()
                .contains(token)
        );
    }
}
