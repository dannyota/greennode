use super::*;
use crate::{
    Config, ConfigOptions, ConfigSource, CredentialsProvider, IamUserAuth, ServiceAccountAuth,
    Token,
};
use std::{
    env,
    ffi::{OsStr, OsString},
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Provider(AtomicUsize);
impl CredentialsProvider for Provider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Token::new(Secret::new("token-canary"), None)) })
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
async fn operation_source_uses_selected_static_token() {
    let config = Config::new(options().with_static_token(Secret::new("toy-token"))).unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(!config.session.refreshable());
    let token = config.session.token(&ctx).await.unwrap();
    assert!(token.token.access_token.expose_secret() == "toy-token");
}

fn options() -> ConfigOptions {
    ConfigOptions::default().with_region("hcm-3".into())
}
fn service() -> ServiceAccountAuth {
    ServiceAccountAuth::new("client-canary".into(), Secret::new("secret-canary"))
}
fn iam() -> IamUserAuth {
    IamUserAuth::new(
        "root-canary".into(),
        "user-canary".into(),
        Secret::new("password-canary"),
    )
}

#[tokio::test]
async fn auth_selection_and_shared_session() {
    let provider = Arc::new(Provider(AtomicUsize::new(0)));
    let opts = options()
        .with_service_account(service())
        .with_iam_user(iam())
        .with_static_token(Secret::new("static-canary"))
        .with_credentials_provider(provider.clone())
        .with_cdn_api_key(Secret::new("cdn-canary"))
        .with_project_id("project-canary".into())
        .with_profile("profile-canary".into())
        .with_config_file("ignored-config".into())
        .with_shared_credentials_file("ignored-credentials".into());
    let config = Config::new(opts.clone()).unwrap();
    assert!(matches!(&config.session.auth, Auth::Provider(_)));
    assert!(Arc::ptr_eq(&config.session, &config.clone().session));
    assert_eq!(config.region(), "hcm-3");
    assert_eq!(
        config.session._project_id.as_deref(),
        Some("project-canary")
    );
    assert_eq!(config.session._profile, "profile-canary");
    assert!(config.profile_setting("region").is_none());
    let (ctx, _) = RequestContext::new();
    config.authenticate(&ctx).await.unwrap();
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    for rendered in [
        format!("{config:?}"),
        format!("{opts:?}"),
        format!("{:?}", service()),
        format!("{:?}", iam()),
    ] {
        for canary in [
            "static-canary",
            "client-canary",
            "secret-canary",
            "password-canary",
            "root-canary",
            "user-canary",
            "cdn-canary",
        ] {
            assert!(!rendered.contains(canary));
        }
    }
    let static_config = Config::new(
        options()
            .with_service_account(service())
            .with_static_token(Secret::new("toy")),
    )
    .unwrap();
    assert!(matches!(&static_config.session.auth, Auth::Static(_)));
    static_config.authenticate(&ctx).await.unwrap();
    let service_config = Config::new(
        options()
            .with_iam_user(iam())
            .with_service_account(service()),
    )
    .unwrap();
    assert!(matches!(
        &service_config.session.auth,
        Auth::ServiceAccount(_)
    ));
    let iam_config = Config::new(options().with_iam_user(iam())).unwrap();
    assert!(matches!(&iam_config.session.auth, Auth::IamUser(_)));
}

#[test]
fn option_validation_and_last_value() {
    assert!(Config::new(ConfigOptions::default()).is_err());
    assert!(Config::new(options()).is_err());
    assert!(
        Config::new(
            options()
                .with_service_account(ServiceAccountAuth::new(String::new(), Secret::new("toy")))
        )
        .is_err()
    );
    assert!(
        Config::new(
            options().with_iam_user(iam().with_signin_base_url("http://example.test".into()))
        )
        .is_err()
    );
    let config = Config::new(
        options()
            .with_region("han-1".into())
            .with_static_token(Secret::new("first"))
            .with_static_token(Secret::new("second")),
    )
    .unwrap();
    assert_eq!(config.region(), "han-1");
    assert!(
        matches!(&config.session.auth, Auth::Static(token) if token.expose_secret() == "second")
    );
    assert!(
        Config::new(
            options()
                .with_static_token(Secret::new("toy"))
                .with_cdn_api_key(Secret::new("bad key"))
        )
        .is_err()
    );
}

struct Home(PathBuf);
impl ConfigSource for Home {
    fn var_os(&self, _: &OsStr) -> Option<OsString> {
        None
    }
    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
}

#[tokio::test]
async fn refuses_legacy_cache_for_builtins_before_io() {
    let home = env::temp_dir().join(format!("greennode-config-cache-{}", std::process::id()));
    std::fs::create_dir_all(home.join(".vngcloud")).unwrap();
    let path = home.join(".vngcloud").join("cache");
    let (ctx, _) = RequestContext::new();
    for opts in [
        options().with_service_account(service()),
        options().with_iam_user(iam()),
    ] {
        let result = Config::load(
            &ctx,
            opts.with_token_cache(path.clone()),
            &Home(home.clone()),
        )
        .await;
        assert!(matches!(result, Err(error) if error.class() == ErrorClass::TokenCache));
        assert!(!path.exists());
    }
    assert!(
        Config::load(
            &ctx,
            options()
                .with_static_token(Secret::new("toy"))
                .with_token_cache(path.clone()),
            &Home(home.clone())
        )
        .await
        .is_ok()
    );
    assert!(!path.exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.join(".vngcloud"), home.join("alias")).unwrap();
        for opts in [
            options().with_service_account(service()),
            options().with_iam_user(iam()),
        ] {
            let result = Config::load(
                &ctx,
                opts.with_token_cache(home.join("alias/cache")),
                &Home(home.clone()),
            )
            .await;
            assert!(matches!(result, Err(error) if error.class() == ErrorClass::TokenCache));
            assert!(!path.exists());
        }
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_service_account_namespace_in_legacy_tree() {
    let home = env::temp_dir().join(format!("greennode-namespace-{}", std::process::id()));
    let legacy = home.join(".vngcloud");
    let cache = home.join("cache");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir(&cache).unwrap();
    std::os::unix::fs::symlink(&legacy, cache.join("service-account-v1")).unwrap();
    let (ctx, _) = RequestContext::new();
    let result = Config::load(
        &ctx,
        options()
            .with_service_account(service())
            .with_token_cache(cache),
        &Home(home.clone()),
    )
    .await;
    assert!(matches!(result, Err(error) if error.class() == ErrorClass::TokenCache));
    assert_eq!(std::fs::read_dir(&legacy).unwrap().count(), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn rechecks_service_namespace_before_cache_io() {
    let home = env::temp_dir().join(format!(
        "greennode-namespace-recheck-{}",
        std::process::id()
    ));
    let legacy = home.join(".vngcloud");
    let cache = home.join("cache");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir(&cache).unwrap();
    let (ctx, _) = RequestContext::new();
    let mut config = Config::load(
        &ctx,
        options()
            .with_service_account(service())
            .with_token_cache(cache.clone()),
        &Home(home.clone()),
    )
    .await
    .unwrap();
    std::os::unix::fs::symlink(&legacy, cache.join("service-account-v1")).unwrap();
    let peer = crate::wire::test_server::Server::start(vec![]);
    if let Auth::ServiceAccount(source) = &mut Arc::get_mut(&mut config.session).unwrap().auth {
        source.set_test_endpoint(peer.origin.clone(), peer.root_der.clone());
    } else {
        panic!("wrong auth kind");
    }
    assert!(
        matches!(config.authenticate(&ctx).await, Err(error) if error.class() == ErrorClass::TokenCache)
    );
    assert_eq!(peer.requests(), 0);
    assert_eq!(std::fs::read_dir(&legacy).unwrap().count(), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn loaded_config_keeps_endpoint_overrides() {
    struct Empty;
    impl ConfigSource for Empty {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
    }
    let (ctx, _) = RequestContext::new();
    let overrides = EndpointOverrides {
        signin: Some("https://signin.test".into()),
        token: Some("https://token.test/path".into()),
        dashboard: Some("https://dashboard.test/".into()),
        ..Default::default()
    };
    let config = Config::load(
        &ctx,
        options()
            .with_iam_user(iam())
            .with_endpoint_overrides(overrides),
        &Empty,
    )
    .await
    .unwrap();
    assert_eq!(
        config.session._endpoints.signin.as_deref(),
        Some("https://signin.test")
    );
    assert_eq!(
        config.session._endpoints.token.as_deref(),
        Some("https://token.test/path")
    );
    assert_eq!(
        config.session._endpoints.dashboard.as_deref(),
        Some("https://dashboard.test/")
    );
}

#[tokio::test]
async fn totp_fn_constructor_adapts_callback() {
    fn callback(
        _: &RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Secret, Error>> + Send + '_>> {
        Box::pin(async { Ok(Secret::new("123456")) })
    }
    let provider = crate::TotpFn::new(callback);
    let (ctx, _) = RequestContext::new();
    assert!(crate::TotpProvider::code(&provider, &ctx).await.is_ok());
}

fn response(
    status: &str,
    location: Option<&str>,
    body: &str,
) -> crate::wire::test_server::Response {
    let mut headers = vec![("Content-Length".into(), body.len().to_string())];
    if let Some(location) = location {
        headers.push(("Location".into(), location.into()));
    }
    crate::wire::test_server::Response::Http {
        status: status.into(),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

#[tokio::test]
async fn authenticate_builtin_sources_against_loopback() {
    use crate::wire::test_server::Server;
    let service_peer = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"access_token":"token-canary","expires_in":120}"#,
    )]);
    let mut config = Config::new(options().with_service_account(service())).unwrap();
    if let Auth::ServiceAccount(source) = &mut Arc::get_mut(&mut config.session).unwrap().auth {
        source.set_test_endpoint(service_peer.origin.clone(), service_peer.root_der.clone());
    } else {
        panic!("wrong auth kind");
    }
    let (ctx, _) = RequestContext::new();
    config.authenticate(&ctx).await.unwrap();
    config.clone().authenticate(&ctx).await.unwrap();
    assert_eq!(service_peer.requests(), 1);

    let page = "<input name=\"_csrf\" value=\"csrf\">";
    let signin = Server::start(vec![
        response("200 OK", None, page),
        response(
            "302 Found",
            Some("https://dashboard.test/return/?code=toy"),
            "",
        ),
    ]);
    let token = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"accessToken":"token-canary","expiresIn":3600}"#,
    )]);
    let auth = iam().with_signin_base_url(signin.origin.clone());
    let overrides = EndpointOverrides {
        signin: Some("https://unused-signin.test".into()),
        dashboard: Some("https://dashboard.test/return/".into()),
        token: Some(format!("{}/token", token.origin)),
        vserver: Some("https://vserver.test/".into()),
        ..Default::default()
    };
    let mut config = Config::new(
        options()
            .with_endpoint_overrides(overrides)
            .with_iam_user(auth),
    )
    .unwrap();
    assert_eq!(
        config.session._endpoints.vserver.as_deref(),
        Some("https://vserver.test/")
    );
    if let Auth::IamUser(source) = &mut Arc::get_mut(&mut config.session).unwrap().auth {
        source.set_test_roots(vec![signin.root_der.clone(), token.root_der.clone()]);
    } else {
        panic!("wrong auth kind");
    }
    let result = config.authenticate(&ctx).await;
    assert!(
        result.is_ok(),
        "{:?}",
        result
            .err()
            .and_then(|e| e.login_error().map(|v| v.reason()))
    );
    config.clone().authenticate(&ctx).await.unwrap();
    assert_eq!(signin.requests(), 2);
    assert_eq!(token.requests(), 1);
}
