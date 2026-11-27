mod screening;

mod monitor;
mod queries;

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
        config
            .session
            .project_id_context(&RequestContext::new().0)
            .await
            .unwrap()
            .as_deref(),
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
        config.session.endpoints().signin.as_deref(),
        Some("https://signin.test")
    );
    assert_eq!(
        config.session.endpoints().token.as_deref(),
        Some("https://token.test/path")
    );
    assert_eq!(
        config.session.endpoints().dashboard.as_deref(),
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
        config.session.endpoints().vserver.as_deref(),
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

#[cfg(feature = "test-support")]
#[tokio::test]
async fn billing_test_trust_refuses_endpoints_outside_the_fixture_listener() {
    use crate::testing::{Server, with_billing_loopback};

    struct Empty;
    impl ConfigSource for Empty {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
    }

    let mut fixture = Server::start(vec![response("200 OK", None, r#"{"code":200,"data":[]}"#)]);

    let other = Server::start(vec![]);
    let address = fixture.address;
    let port = address.port();
    fixture.address = other.address;
    fixture.root_der = other.root_der.clone();
    let (ctx, _) = RequestContext::new();
    for endpoint in [
        format!("https://remote.test:{port}"),
        format!("https://192.0.2.10:{port}"),
        other.origin.clone(),
    ] {
        let options = with_billing_loopback(
            options()
                .with_static_token(Secret::new("test-bearer"))
                .with_endpoint_overrides(EndpointOverrides {
                    billing: Some(endpoint),
                    ..Default::default()
                }),
            &fixture,
        );
        assert!(
            matches!(Config::new(options.clone()), Err(error) if error.class() == ErrorClass::InvalidConfig)
        );
        assert!(
            matches!(Config::load(&ctx, options, &Empty).await, Err(error) if error.class() == ErrorClass::InvalidConfig)
        );
    }

    let valid = with_billing_loopback(
        options()
            .with_static_token(Secret::new("test-bearer"))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(format!("https://{address}")),
                ..Default::default()
            }),
        &fixture,
    );
    let config = Config::load(&ctx, valid, &Empty).await.unwrap();
    let output = crate::billing::Client::new(&config)
        .list_budgets(&ctx, crate::billing::ListBudgetsInput::default())
        .await
        .unwrap();
    assert!(output.items.is_empty());
    assert_eq!(fixture.requests(), 1);
    assert_eq!(other.requests(), 0);
}

#[tokio::test]
async fn billing_withholds_configured_and_exchanged_service_account_credentials() {
    use crate::billing::{Client, GetBudgetInput};
    use crate::wire::test_server::Server;

    let login = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"access_token":"token-canary","expires_in":120}"#,
    )]);
    let encoded = data_encoding::BASE64.encode(b"client-canary:secret-canary");
    let sensitive = ["secret-canary", "cdn-canary", encoded.as_str()];
    let billing = Server::start(
        sensitive
            .iter()
            .copied()
            .chain(["monthly"])
            .map(|name| {
                response(
                    "200 OK",
                    None,
                    &format!(r#"{{"code":200,"data":{{"uuid":"budget-1","name":"{name}"}}}}"#),
                )
            })
            .collect(),
    );
    let mut config = Config::new(
        options()
            .with_service_account(service())
            .with_cdn_api_key(Secret::new("cdn-canary"))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(billing.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::ServiceAccount(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_endpoint(login.origin.clone(), login.root_der.clone());
    session.set_test_product_root(billing.root_der.clone());
    let client = Client::new(&config);
    let (ctx, _) = RequestContext::new();
    for fragment in sensitive {
        let Err(failure) = client
            .get_budget(&ctx, GetBudgetInput::new("budget-1"))
            .await
        else {
            panic!("credential reflection returned a budget");
        };
        assert!(failure.partial().is_none());
        assert!(
            failure
                .error()
                .api_error()
                .is_some_and(|api| api.code() == "InvalidResponse")
        );
        assert!(!format!("{failure:?} {}", failure.error()).contains(fragment));
    }
    let output = client
        .get_budget(&ctx, GetBudgetInput::new("budget-1"))
        .await
        .unwrap();
    assert_eq!(output.budget.name, "monthly");
    assert_eq!(login.requests(), 1);
}

#[tokio::test]
async fn billing_withholds_iam_secrets_and_signin_material_after_cached_login() {
    use crate::billing::{Client, GetBudgetInput};
    use crate::wire::test_server::{Response, Server};

    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    const PASSWORD: &str = "test-iam-password";
    const COOKIE: &str = "test-iam-cookie";
    const CODE: &str = "test-iam-auth-code";

    struct Source;
    impl ConfigSource for Source {
        fn var_os(&self, name: &OsStr) -> Option<OsString> {
            let value = match name.to_str()? {
                "GREENNODE_ROOT_EMAIL" => "test-iam-root",
                "GREENNODE_USERNAME" => "test-iam-user",
                "GREENNODE_PASSWORD" => PASSWORD,
                "GREENNODE_TOTP_SECRET" => {
                    return Some(format!("otpauth://totp/example?secret={SEED}").into());
                }
                _ => return None,
            };
            Some(value.into())
        }
    }

    let page = "<input name=\"_csrf\" value=\"test-iam-csrf\">";
    let signin = Server::start(vec![
        Response::Http {
            status: "200 OK".into(),
            headers: vec![
                ("Content-Length".into(), page.len().to_string()),
                (
                    "Set-Cookie".into(),
                    format!("session={COOKIE}; Path=/; Secure"),
                ),
            ],
            body: page.as_bytes().to_vec(),
        },
        response("302 Found", Some("/ap/auth/iam/google"), ""),
        response("200 OK", None, page),
        response(
            "302 Found",
            Some(&format!("https://dashboard.test/return/?code={CODE}")),
            "",
        ),
    ]);
    let token = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"accessToken":"test-iam-bearer","expiresIn":3600}"#,
    )]);
    let sensitive = [PASSWORD, SEED, COOKIE, CODE];
    let billing = Server::start(
        sensitive
            .into_iter()
            .chain(["monthly"])
            .map(|name| response(
                "200 OK", None,
                &format!(r#"{{"code":200,"data":{{"uuid":"budget-1","name":"{name}","limitAmount":100}}}}"#),
            ))
            .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let mut config = Config::load(
        &ctx,
        options().with_endpoint_overrides(EndpointOverrides {
            signin: Some(signin.origin.clone()),
            dashboard: Some("https://dashboard.test/return".into()),
            token: Some(format!("{}/token", token.origin)),
            billing: Some(billing.origin.clone()),
            ..Default::default()
        }),
        &Source,
    )
    .await
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::IamUser(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_roots(vec![signin.root_der.clone(), token.root_der.clone()]);
    session.set_test_product_root(billing.root_der.clone());
    config.authenticate(&ctx).await.unwrap();
    let client = Client::new(&config);
    for credential in sensitive {
        let Err(failure) = client
            .get_budget(&ctx, GetBudgetInput::new("budget-1"))
            .await
        else {
            panic!("credential reflection returned a budget");
        };
        assert!(failure.partial().is_none());
        assert!(
            failure
                .error()
                .api_error()
                .is_some_and(|api| api.code() == "InvalidResponse")
        );
        assert!(!format!("{failure:?} {}", failure.error()).contains(credential));
    }
    let output = client
        .get_budget(&ctx, GetBudgetInput::new("budget-1"))
        .await
        .unwrap();
    assert_eq!(output.budget.name, "monthly");
    assert_eq!(signin.requests(), 4);
    assert_eq!(token.requests(), 1);
}

mod pricing;
