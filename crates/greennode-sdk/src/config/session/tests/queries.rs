use super::*;

fn project_response(body: &[u8]) -> crate::wire::test_server::Response {
    crate::wire::test_server::Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn project_config(server: &crate::wire::test_server::Server, id: Option<&str>) -> Config {
    let mut opts = options()
        .with_static_token(Secret::new("toy-token"))
        .with_endpoint_overrides(EndpointOverrides {
            vserver: Some(server.origin.clone()),
            ..Default::default()
        });
    if let Some(id) = id {
        opts = opts.with_project_id(id.into());
    }
    let config = Config::new(opts).unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    config
}

#[tokio::test]
async fn configured_id_skips_selection_but_metadata_is_read() {
    let server = crate::wire::test_server::Server::start(vec![
        project_response(
            br#"{"projects":[{"projectId":"other"},{"projectId":"given","userId":7}]}"#,
        ),
        project_response(br#"{"projects":[{"projectId":"other"}]}"#),
    ]);
    let config = project_config(&server, Some("given"));
    let session = config.session();
    let (ctx, _) = RequestContext::new();

    assert_eq!(
        session.project_id_context(&ctx).await.unwrap().as_deref(),
        Some("given")
    );
    assert_eq!(session.require_project_id(&ctx).await.unwrap(), "given");
    assert_eq!(server.requests(), 0);

    let project = session.require_project(&ctx).await.unwrap();
    assert_eq!(project.id, "given");
    assert_eq!(project.user_id, 7);
    assert_eq!(project.region, "hcm-3");

    let project = session.require_project(&ctx).await.unwrap();
    assert_eq!(project.id, "given");
    assert_eq!(project.region, "hcm-3");
    assert_eq!(project.user_id, 0);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn project_cache_is_shared_and_cached_access_respects_cancellation() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = crate::wire::test_server::Server::start(vec![
        crate::wire::test_server::Response::StallBeforeHead(started),
        project_response(br#"{"projects":[{"projectId":"selected"}]}"#),
    ]);
    let config = project_config(&server, None);
    let clone = config.clone();
    let (ctx, cancel) = RequestContext::new();
    assert_eq!(
        config.session().project_id_context(&ctx).await.unwrap(),
        None
    );
    assert_eq!(server.requests(), 0);

    let holder = config.session().require_project_id(&ctx);
    tokio::pin!(holder);
    tokio::select! {
        _ = &mut holder => panic!("holder finished early"),
        result = ready => result.unwrap(),
    }
    let (wait_ctx, wait_cancel) = RequestContext::new();
    let cached = clone.session().project_id_context(&wait_ctx);
    tokio::pin!(cached);
    tokio::select! {
        _ = &mut cached => panic!("cached access skipped the discovery gate"),
        _ = tokio::task::yield_now() => {}
    }
    wait_cancel.cancel();
    assert_eq!(cached.await.unwrap_err().class(), ErrorClass::Cancelled);
    assert_eq!(server.requests(), 1);

    let (next_ctx, _) = RequestContext::new();
    let next = clone.session().require_project_id(&next_ctx);
    cancel.cancel();
    let (first, second) = tokio::join!(holder, next);
    assert_eq!(first.unwrap_err().class(), ErrorClass::Cancelled);
    assert_eq!(second.unwrap(), "selected");
    assert_eq!(
        config
            .session()
            .project_id_context(&next_ctx)
            .await
            .unwrap()
            .as_deref(),
        Some("selected")
    );
    assert_eq!(server.requests(), 2);
}

#[test]
fn session_reports_auth_provenance_and_default_endpoints_offline() {
    let provider = Arc::new(Provider(AtomicUsize::new(0)));
    let sa = options()
        .with_iam_user(iam())
        .with_service_account(service());
    let token = sa.clone().with_static_token(Secret::new("toy-token"));
    for (opts, kind) in [
        (options().with_iam_user(iam()), AuthKind::IamUser),
        (sa, AuthKind::ServiceAccount),
        (token.clone(), AuthKind::Static),
        (
            token.with_credentials_provider(provider.clone()),
            AuthKind::Custom,
        ),
    ] {
        let config = Config::new(opts).unwrap();
        let session = config.session();
        assert_eq!(session.auth_kind(), kind);
        assert_eq!(
            session.uses_default_auth_endpoints(),
            kind == AuthKind::IamUser
        );
        assert!(!session.endpoint_overrides_used());
    }
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);

    let config = Config::new(
        options()
            .with_region("han-1".into())
            .with_iam_user(iam())
            .with_endpoint_overrides(EndpointOverrides {
                vks: Some("https://VKS-HAN-1.console.greennode.ai:443/vks-api/".into()),
                ..Default::default()
            }),
    )
    .unwrap();
    let session = config.session();
    assert_eq!(
        session.default_endpoints().vks.as_deref(),
        Some("https://vks-han-1.console.greennode.ai/vks-api/")
    );
    assert_eq!(
        session.endpoints().vks.as_deref(),
        session.default_endpoints().vks.as_deref()
    );
    let vks = session.endpoints().vks.as_ref().unwrap();
    assert!(vks.is_explicit_override());
    assert_ne!(
        vks.configured_url(),
        session
            .default_endpoints()
            .vks
            .as_ref()
            .unwrap()
            .configured_url()
    );
    assert!(
        !session
            .endpoints()
            .vnetwork
            .as_ref()
            .unwrap()
            .is_explicit_override()
    );
    assert!(session.endpoint_overrides_used());
    assert!(session.uses_default_auth_endpoints());

    for opts in [
        options().with_iam_user(iam().with_signin_base_url("https://signin.example.test".into())),
        options()
            .with_iam_user(iam())
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some("https://dashboard.example.test/".into()),
                ..Default::default()
            }),
        options()
            .with_iam_user(iam())
            .with_endpoint_overrides(EndpointOverrides {
                signin: Some("https://SIGNIN.greennode.ai:443".into()),
                ..Default::default()
            }),
    ] {
        let config = Config::new(opts).unwrap();
        assert!(config.session().endpoint_overrides_used());
        assert!(!config.session().uses_default_auth_endpoints());
    }
}
