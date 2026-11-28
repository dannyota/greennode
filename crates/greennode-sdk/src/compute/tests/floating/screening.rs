use super::*;

struct Switch {
    calls: AtomicUsize,
    at: usize,
    token: &'static str,
}
impl CredentialsProvider for Switch {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(Token::new(
                Secret::new(if self.calls.fetch_add(1, Ordering::SeqCst) < self.at {
                    CANARY
                } else {
                    self.token
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

#[tokio::test]
async fn regression_screened_rejection_keeps_userdata_withheld_and_truncation_ambiguous() {
    for truncated in [false, true] {
        let order = if truncated {
            Response::RawClose(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 100\r\n\r\n{".to_vec())
        } else {
            http("403 Forbidden", r#"{"message":"peer text"}"#)
        };
        let server = TestServer::start(vec![
            response(LIST),
            response(GROUP),
            response(SAFE),
            response(QUOTE),
            response(GROUP),
            response(SAFE),
            order,
        ]);
        let (ctx, _) = RequestContext::new();
        let mut input = floating_input();
        input.ssh_key_id.clear();
        input.user_data = Some(Secret::new("toy cloud init"));
        let options = ConfigOptions::default().with_credentials_provider(Arc::new(Switch {
            calls: AtomicUsize::new(0),
            at: 6,
            token: "403",
        }));
        let failure = floating_client(&server, options)
            .create_server(&ctx, input)
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), None);
        if truncated {
            assert!(failure.error().detail::<crate::RecoveryAdvice>().is_some());
        } else {
            assert_eq!(
                api.message(),
                "server message withheld: it may quote user data"
            );
        }
    }
}

#[tokio::test]
async fn regression_group_receipt_rechecked_before_open_login_refusal() {
    let group = r#"{"data":{"id":"sg-1","name":"future-bearer"}}"#;
    let open = SAFE.replace("192.0.2.8/32", "0.0.0.0/0");
    let server = TestServer::start(vec![response(LIST), response(group), response(&open)]);
    let (ctx, _) = RequestContext::new();
    let options = ConfigOptions::default().with_credentials_provider(Arc::new(Switch {
        calls: AtomicUsize::new(0),
        at: 2,
        token: "future-bearer",
    }));
    let failure = floating_client(&server, options)
        .create_server(&ctx, floating_input())
        .await
        .unwrap_err();
    assert!(failure.error().detail::<OpenLogin>().is_none());
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn regression_group_receipt_rechecked_before_snapshot_refusal() {
    let group = r#"{"data":{"id":"sg-1","name":"future-bearer"}}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(QUOTE),
        response(group),
        response(SAFE),
    ]);
    let (ctx, _) = RequestContext::new();
    let options = ConfigOptions::default().with_credentials_provider(Arc::new(Switch {
        calls: AtomicUsize::new(0),
        at: 5,
        token: "future-bearer",
    }));
    let mut input = floating_input();
    input.allow_open_login = true;
    let failure = floating_client(&server, options)
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().detail::<UnexpectedStatus>().is_none());
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn regression_invalid_pagination_is_usage_and_alias_collision_invalid_response() {
    for (inventory, usage) in [
        (
            r#"{"listData":[],"page":1,"pageSize":0,"totalPage":1,"totalItem":0}"#,
            true,
        ),
        (
            r#"{"listData":[],"items":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":0}"#,
            false,
        ),
    ] {
        let server = TestServer::start(vec![response(inventory)]);
        let (ctx, _) = RequestContext::new();
        let mut input = floating_input();
        input.attach_floating_ip = false;
        let failure = floating_client(&server, key_options())
            .create_server(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(failure.error().has_class(ErrorClass::InvalidInput), usage);
        if !usage {
            assert_eq!(
                failure.error().api_error().unwrap().code(),
                "InvalidResponse"
            );
        }
    }
}

struct LongRotating(AtomicUsize);
impl CredentialsProvider for LongRotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            let index = self.0.fetch_add(1, Ordering::SeqCst).min(255);
            let mut state = index as u64 + 1;
            let token: String = (0..1024)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    char::from(
                        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
                            [(state % 64) as usize],
                    )
                })
                .collect();
            Ok(Token::new(Secret::new(token), None))
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

#[tokio::test]
async fn floating_accepts_256_distinct_kibibyte_bearers() {
    let mut input = floating_input();
    input.no_wait = true;
    input.security_group_ids = (0..64).map(|index| format!("sg-{index}")).collect();
    let mut replies = vec![response(LIST)];
    for pass in 0..2 {
        if pass == 1 {
            replies.push(response(QUOTE));
        }
        for id in &input.security_group_ids {
            replies.push(response(&serde_json::json!({"data":{"id":id}}).to_string()));
            replies.push(response(SAFE));
        }
    }
    replies.push(http("202 Accepted", ACK));
    let server = TestServer::start(replies);
    let provider = Arc::new(LongRotating(AtomicUsize::new(0)));
    let options = ConfigOptions::default()
        .with_clock(Arc::new(crate::TestClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
        )))
        .with_credentials_provider(provider.clone());
    let (ctx, _) = RequestContext::new();
    let output = floating_client(&server, options)
        .create_server(&ctx, input)
        .await
        .unwrap();
    assert_eq!(output.server.uuid, "server-1");
    assert_eq!(provider.0.load(Ordering::SeqCst), 259);
}

struct WideRotating(AtomicUsize);
impl CredentialsProvider for WideRotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            let index = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Token::new(
                Secret::new(format!("wide-{index}-{}", "x".repeat(60 << 10))),
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

#[tokio::test]
async fn credential_byte_limit_refuses_before_next_request() {
    let group2 = r#"{"data":{"id":"sg-2"}}"#;
    let server = TestServer::start(vec![
        response(LIST),
        response(GROUP),
        response(SAFE),
        response(group2),
        response(SAFE),
        response(QUOTE),
        response(GROUP),
        response(SAFE),
        response(group2),
        response(SAFE),
        http("202 Accepted", ACK),
    ]);
    let provider = Arc::new(WideRotating(AtomicUsize::new(0)));
    let options = ConfigOptions::default()
        .with_clock(Arc::new(crate::TestClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
        )))
        .with_credentials_provider(provider.clone());
    let mut input = floating_input();
    input.no_wait = true;
    input.security_group_ids.push("sg-2".into());
    let (ctx, _) = RequestContext::new();
    let failure = floating_client(&server, options)
        .create_server(&ctx, input)
        .await
        .unwrap_err();
    assert!(failure.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(failure.write_dispatch(), crate::WriteDispatch::NotSent);
    assert_eq!(server.heads().len(), 4);
    assert_eq!(provider.0.load(Ordering::SeqCst), 5);
}

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
