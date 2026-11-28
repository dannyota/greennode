use super::*;

#[tokio::test]
async fn resource_rows_refuse_credentials_acquired_since_the_list() {
    let account_body = r#"{"code":200,"data":{"accountId":123456}}"#;
    for (field, value, late_bearer) in [
        ("artifactName", json!(SECOND), SECOND),
        ("artifactId", json!(SECOND), SECOND),
        ("cost", json!(5.050505e30), "PPPP"),
    ] {
        let resource = json!({"code":200,"data":{"data":[{(field):value}]}}).to_string();
        let server = Server::start(vec![
            reply("200 OK", resource),
            reply("200 OK", account_body),
        ]);
        let config = rotating_config(&server, [FIRST, late_bearer]);
        let (ctx, _) = RequestContext::new();
        let mut evidence = Evidence::new();
        let retained = list(config.session(), &ctx, &mut evidence).await.unwrap();
        assert!(retained.rows(&evidence).is_ok());
        let _ = account(config.session(), &ctx, &mut evidence)
            .await
            .unwrap();
        assert!(evidence.text_reflects(late_bearer));
        let error = refusal(retained.rows(&evidence));
        invalid_at(&error, 200);
        assert_eq!(server.heads().len(), 2);
    }
}

#[tokio::test]
async fn raw_numeric_fields_screen_encoded_rendered_spellings() {
    for (field, number, bearer) in [
        ("status", "7.8e1", "x"),
        ("statusUI", "5.050505e30", "PPPP"),
    ] {
        let body =
            format!(r#"{{"code":200,"data":{{"data":[{{"{field}":{{"value":{number}}}}}]}}}}"#);
        let server = Server::start(vec![reply("200 OK", body)]);
        let config = config(
            &server,
            ConfigOptions::default().with_static_token(Secret::new(bearer)),
        );
        let (ctx, _) = RequestContext::new();
        let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        invalid_at(&error, 200);
    }
}

#[test]
fn sign_in_errors_screen_typed_reasons_and_statuses() {
    let op = operation(LIST, "gateway/api/v1/resources", Method::GET);
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("synthetic-auth-reason"));
    evidence.add_secret(&Secret::new("418"));
    let iam = Error::new(ErrorClass::Auth).with_login(crate::LoginError::new(
        "sign-in failed with synthetic-auth-reason",
        Some(418),
        true,
    ));
    let screened = withheld_error(&op, iam, &evidence);
    let detail = screened.login_error().unwrap();
    assert!(!detail.reason().contains("synthetic-auth-reason"));
    assert_eq!(detail.status(), None);
    assert!(detail.captcha_suspected());
}

struct RejectRefresh(AtomicUsize);

impl crate::wire::product::sender::BearerSource for RejectRefresh {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<crate::wire::product::sender::BearerToken, Error>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(crate::wire::product::sender::BearerToken {
                    token: Token::new(Secret::new(FIRST), None),
                    obtained_at: None,
                    signin_credentials: None,
                });
            }
            Err(Error::new(ErrorClass::Auth).with_service_login(
                crate::ServiceAccountLoginError::new(
                    "token exchange failed with synthetic-auth-reason",
                    Some(418),
                ),
            ))
        })
    }

    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn get_401_refresh_failure_preserves_the_screened_service_account_detail() {
    let server = Server::start(vec![reply("401 Unauthorized", "")]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let op = operation(LIST, "gateway/api/v1/resources", Method::GET);
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("synthetic-auth-reason"));
    evidence.add_secret(&Secret::new("418"));
    let session = config.session();
    let endpoint = session.endpoints().billing.as_ref().unwrap();
    let mut jitter = || 0;
    let runtime = session.runtime(&ctx, &mut jitter).unwrap();
    let source = RejectRefresh(AtomicUsize::new(0));
    let failure = refusal(
        operation::execute_raw(
            runtime,
            endpoint,
            &op,
            &Options::DEFAULT,
            &[],
            &[],
            RouteAuth::Bearer(&source),
            Bytes::new(),
            &mut evidence,
        )
        .await,
    );
    let (error, _, _) = failure.into_parts();
    let screened = withheld_error(&op, error, &evidence);
    assert!(screened.has_identity(ErrorClass::Auth));
    assert!(screened.api_error().is_none());
    assert!(screened.http_status().is_none());
    let detail = screened.service_account_login_error().unwrap();
    assert!(!detail.reason().contains("synthetic-auth-reason"));
    assert_eq!(detail.status(), None);
    assert_eq!(source.0.load(Ordering::SeqCst), 2);
    let heads = server.heads();
    assert_eq!(heads.len(), 1);
    assert!(heads[0].starts_with(b"GET /gateway/api/v1/resources "));
}

#[tokio::test]
async fn truncated_error_statuses_keep_the_body_read_retry_hint() {
    for (head, status) in [
        ("429 Too Many Requests", 429),
        ("503 Service Unavailable", 503),
    ] {
        let bytes = format!("HTTP/1.1 {head}\r\nContent-Length: 128\r\n\r\n{{").into_bytes();
        let server = Server::start(vec![Response::RawClose(bytes)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        let detail = error.api_error().unwrap();
        assert_eq!(detail.status(), Some(status));
        assert!(!detail.retryable());
        assert_eq!(server.heads().len(), 1);
    }
}

#[tokio::test]
async fn failed_renewal_dial_preserves_empty_code_and_retry_hint() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(FIRST))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(format!("https://localhost:{}", address.port())),
                ..Default::default()
            }),
    )
    .unwrap();
    let (ctx, _) = RequestContext::new();
    let account = AccountId(Secret::new("123456"));
    let failure = refusal(
        put(
            config.session(),
            &ctx,
            "put-test",
            &account,
            setting(),
            &mut Evidence::new(),
        )
        .await,
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let detail = failure.error().api_error().unwrap();
    assert_eq!(detail.status(), None);
    assert!(detail.code().is_empty());
    assert!(detail.retryable());
}
