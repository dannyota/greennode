use super::*;
use crate::monitor::{
    Client, CreateSyntheticQuotaInput, EnsureSyntheticQuotaInput, RecoverSyntheticQuotaInput,
    RenewSyntheticQuotaInput,
};
use crate::wire::test_server::Server;
use crate::{Failure, WriteDispatch};
use serde::Serialize;

const PEER_TEXT: &str = "peer-diagnostic-canary";
const CSRF: &str = "fixture-csrf-canary";

fn service_account_config(options: ConfigOptions, login: &Server, monitor: &Server) -> Config {
    let mut config = Config::new(
        options
            .with_service_account(service())
            .with_product_loopback(crate::testing::Product::Monitor, monitor, true),
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
    config
}

fn iam_user_config(signin: &Server, monitor: &Server) -> Config {
    let mut config = Config::new(
        options()
            .with_iam_user(iam().with_signin_base_url(signin.origin.clone()))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/return", signin.origin)),
                token: Some(format!("{}/token", signin.origin)),
                ..Default::default()
            })
            .with_product_loopback(crate::testing::Product::Monitor, monitor, true),
    )
    .unwrap();
    let session = Arc::get_mut(&mut config.session).unwrap();
    let Auth::IamUser(source) = &mut session.auth else {
        panic!("wrong auth kind");
    };
    source.set_test_roots(vec![signin.root_der.clone()]);
    config
}

fn assert_preflight(
    failure: &Failure<impl Serialize + std::fmt::Debug>,
    operation: &str,
    code: &str,
    status: u16,
) {
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    let partial = failure
        .partial()
        .expect("missing synthetic preflight envelope");
    let output = serde_json::to_value(partial).unwrap();
    let error = failure.error();
    let visible = format!("{failure:?} {error} {output}");
    let authorization = data_encoding::BASE64.encode(b"client-canary:secret-canary");
    for fragment in [
        PEER_TEXT,
        "client-canary",
        "secret-canary",
        &authorization,
        "root-canary",
        "user-canary",
        "password-canary",
        CSRF,
    ] {
        assert!(!visible.contains(fragment));
    }
    assert_eq!(output["Sent"], "no");
    assert_eq!(output["Stage"], "preflight");
    assert_eq!(output["Error"]["Code"], code);
    assert_eq!(output["Error"]["Operation"], operation);
    assert_eq!(output["Error"]["Status"], status);
    assert!(output["Quota"].is_null());
    assert_eq!(output["OrderID"], "");
    assert_eq!(output["PaymentPending"], false);
}

struct RefusingProvider(AtomicUsize);

impl CredentialsProvider for RefusingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(Error::provider_failed()) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        panic!("a failed initial acquisition cannot invalidate a bearer");
    }
}

#[cfg(unix)]
struct UnsafeCache(PathBuf);

#[cfg(unix)]
impl UnsafeCache {
    fn new() -> Self {
        use std::os::unix::fs::{DirBuilderExt, symlink};

        let path = env::temp_dir().canonicalize().unwrap().join(format!(
            "greennode-monitor-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let cache = Self(path);
        let store = crate::cache::store::Store::create(&cache.0).unwrap();
        store.create_child("service-account-v1").unwrap();
        let auth = service();
        let stem = crate::cache::service_account_stem(
            "default",
            auth.client_id.expose_secret(),
            "https://iamapis.vngcloud.vn/accounts-api/v2/auth/token",
            &auth.client_secret,
        );
        let lock = cache
            .0
            .join("service-account-v1")
            .join(format!("{stem}.lock"));
        symlink("missing", lock).unwrap();
        cache
    }
}

#[cfg(unix)]
impl Drop for UnsafeCache {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

macro_rules! synthetic_preflight_cases {
    ($( $operation:literal => $method:ident($input:expr) ),+ $(,)?) => {
        mod service_account {
            use super::*;

            $(
                #[tokio::test]
                async fn $method() {
                    let login = Server::start(vec![response("403 Forbidden", None, PEER_TEXT)]);
                    let monitor = Server::start(vec![]);
                    let client = Client::new(&service_account_config(options(), &login, &monitor));
                    let (ctx, _) = RequestContext::new();
                    let failure = client.$method(&ctx, $input).await.unwrap_err();
                    assert_eq!(login.requests(), 1);
                    assert!(login.heads()[0].starts_with(b"POST /accounts-api/v2/auth/token "));
                    assert_eq!(monitor.requests(), 0);
                    assert_preflight(&failure, $operation, "ServiceAccountLoginFailed", 403);
                    assert_eq!(failure.error().class(), ErrorClass::Auth);
                    let detail = failure.error().service_account_login_error().unwrap();
                    assert_eq!(detail.status(), Some(403));
                    assert_eq!(detail.reason(), "token endpoint rejected request");
                }
            )+
        }

        mod iam_user {
            use super::*;

            $(
                #[tokio::test]
                async fn $method() {
                    let page = format!(r#"<input name="_csrf" value="{CSRF}">"#);
                    let signin = Server::start(vec![
                        response("200 OK", None, &page),
                        response("403 Forbidden", None, PEER_TEXT),
                    ]);
                    let monitor = Server::start(vec![]);
                    let client = Client::new(&iam_user_config(&signin, &monitor));
                    let (ctx, _) = RequestContext::new();
                    let failure = client.$method(&ctx, $input).await.unwrap_err();
                    assert_eq!(signin.requests(), 2);
                    let heads = signin.heads();
                    assert!(heads[0].starts_with(b"GET /ap/auth/iam/login?"));
                    assert!(heads[1].starts_with(b"POST /ap/auth/iam/login?"));
                    assert_eq!(monitor.requests(), 0);
                    assert_preflight(&failure, $operation, "RequestFailed", 0);
                    assert_eq!(failure.error().class(), ErrorClass::Auth);
                    let detail = failure.error().login_error().unwrap();
                    assert_eq!(detail.status(), Some(403));
                    assert_eq!(detail.reason(), "signin submission");
                }
            )+
        }

        #[cfg(unix)]
        mod unsafe_token_cache {
            use super::*;

            $(
                #[tokio::test]
                async fn $method() {
                    let cache = UnsafeCache::new();
                    let login = Server::start(vec![]);
                    let monitor = Server::start(vec![]);
                    let options = options().with_token_cache(cache.0.clone());
                    let client = Client::new(&service_account_config(options, &login, &monitor));
                    let (ctx, _) = RequestContext::new();
                    let failure = client.$method(&ctx, $input).await.unwrap_err();
                    assert_eq!(login.requests(), 0);
                    assert_eq!(monitor.requests(), 0);
                    assert_eq!(failure.error().class(), ErrorClass::TokenCache);
                    assert!(failure.error().service_account_login_error().is_none());
                    assert_preflight(&failure, $operation, "TokenCacheFailed", 0);
                }
            )+
        }

        mod custom_provider {
            use super::*;

            $(
                #[tokio::test]
                async fn $method() {
                    let monitor = Server::start(vec![]);
                    let provider = Arc::new(RefusingProvider(AtomicUsize::new(0)));
                    let config = Config::new(
                        options()
                            .with_credentials_provider(provider.clone())
                            .with_product_loopback(crate::testing::Product::Monitor, &monitor, true),
                    )
                    .unwrap();
                    let (ctx, _) = RequestContext::new();
                    let failure = Client::new(&config).$method(&ctx, $input).await.unwrap_err();
                    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
                    assert_eq!(monitor.requests(), 0);
                    assert_eq!(failure.error().class(), ErrorClass::Auth);
                    assert!(failure.error().login_error().is_none());
                    assert!(failure.error().service_account_login_error().is_none());
                    assert_preflight(&failure, $operation, "RequestFailed", 0);
                }
            )+
        }
    };
}

synthetic_preflight_cases! {
    "monitor.CreateSyntheticQuota" => create_synthetic_quota(CreateSyntheticQuotaInput::new("class-1", "unit_1", 10)),
    "monitor.RenewSyntheticQuota" => renew_synthetic_quota(RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z")),
    "monitor.RecoverSyntheticQuota" => recover_synthetic_quota(RecoverSyntheticQuotaInput::new("quota-1")),
    "monitor.EnsureSyntheticQuota" => ensure_synthetic_quota(EnsureSyntheticQuotaInput::default()),
}
