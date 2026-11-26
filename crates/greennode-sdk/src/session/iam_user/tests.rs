use super::*;
use crate::SystemClock;
use crate::wire::test_server::{Response, Server};
#[cfg(unix)]
use std::env;

fn response(status: &str, location: Option<&str>, body: &str) -> Response {
    let mut headers = vec![("Content-Length".into(), body.len().to_string())];
    if let Some(location) = location {
        headers.push(("Location".into(), location.into()));
    }
    Response::Http {
        status: status.into(),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

fn peers(totp: bool) -> (Server, Server) {
    let page = "<input name=\"_csrf\" value=\"csrf\">";
    let mut responses = vec![response("200 OK", None, page)];
    if totp {
        responses.push(response("302 Found", Some("/ap/auth/iam/google"), ""));
        responses.push(response("200 OK", None, page));
        responses.push(response(
            "302 Found",
            Some("https://dashboard.test/return?code=toy"),
            "",
        ));
    } else {
        responses.push(response(
            "302 Found",
            Some("https://dashboard.test/return?code=toy"),
            "",
        ));
    }
    let signin = Server::start(responses);
    let token = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"accessToken":"toy-token","expiresIn":3600}"#,
    )]);
    (signin, token)
}

fn source(
    signin: &Server,
    token: &Server,
    directory: Option<PathBuf>,
    timeout: Duration,
) -> IamUser {
    let mut result = IamUser::new(Options {
        root_email: "root".into(),
        username: "user".into(),
        password: Secret::new("password"),
        seed: Some(Secret::new("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")),
        provider: None,
        profile: "default".into(),
        directory,
        home: None,
        signin: url::Url::parse(&signin.origin).unwrap(),
        dashboard: url::Url::parse("https://dashboard.test/return").unwrap(),
        token_url: url::Url::parse(&format!("{}/token", token.origin)).unwrap(),
        clock: Arc::new(SystemClock),
        timeout,
    });
    result.roots = Some(vec![signin.root_der.clone(), token.root_der.clone()]);
    result
}

#[cfg(unix)]
struct Dir(PathBuf);
#[cfg(unix)]
impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-iam-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
#[cfg(unix)]
impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn only_old_exact_bearer_is_rejected() {
    let now = SystemClock.monotonic_now();
    let token = Token::new(Secret::new("toy"), None);
    let young = Cached {
        token: token.clone(),
        obtained: ObtainedAt::InProcess(now),
        expires_monotonic: None,
    };
    let old = Cached {
        token,
        obtained: ObtainedAt::InProcess(now - Duration::from_secs(30)),
        expires_monotonic: None,
    };
    assert!(!young.reject(&Secret::new("toy"), &SystemClock));
    assert!(!old.reject(&Secret::new("other"), &SystemClock));
    assert!(old.reject(&Secret::new("toy"), &SystemClock));
}

#[tokio::test]
async fn cold_signin_and_memory_cache() {
    let (signin, token) = peers(false);
    let source = source(&signin, &token, None, Duration::from_secs(5));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(signin.requests(), 2);
    assert_eq!(token.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn another_source_reuses_disk_cache() {
    let dir = Dir::new();
    let (signin, token) = peers(false);
    let first = source(
        &signin,
        &token,
        Some(dir.0.join("cache")),
        Duration::from_secs(5),
    );
    let second = source(
        &signin,
        &token,
        Some(dir.0.join("cache")),
        Duration::from_secs(5),
    );
    let (ctx, _) = RequestContext::new();
    assert!(first.token(&ctx).await.is_ok());
    assert!(second.token(&ctx).await.is_ok());
    assert_eq!(signin.requests(), 2);
    assert_eq!(token.requests(), 1);
}

#[tokio::test]
async fn concurrent_callers_share_signin() {
    let (signin, token) = peers(false);
    let source = Arc::new(source(&signin, &token, None, Duration::from_secs(5)));
    let (ctx, _) = RequestContext::new();
    let a = source.clone();
    let b = source.clone();
    let other = ctx.clone();
    let first = tokio::spawn(async move { a.token(&ctx).await.is_ok() });
    let second = tokio::spawn(async move { b.token(&other).await.is_ok() });
    assert!(first.await.unwrap());
    assert!(second.await.unwrap());
    assert_eq!(signin.requests(), 2);
    assert_eq!(token.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn fresh_totp_is_submitted_and_marker_written() {
    let dir = Dir::new();
    let (signin, token) = peers(true);
    let path = dir.0.join("cache");
    let source = source(&signin, &token, Some(path.clone()), Duration::from_secs(5));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(signin.requests(), 4);
    assert_eq!(token.requests(), 1);
    let stem = crate::cache::iam_stem(
        &source.profile,
        &source.root_email,
        &source.username,
        source.signin.as_str().trim_end_matches('/'),
        source.token_url.as_str(),
    );
    assert!(path.join(format!("{stem}.totp")).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn used_marker_prevents_totp_submission() {
    let dir = Dir::new();
    let (signin, token) = peers(true);
    let path = dir.0.join("cache");
    std::fs::create_dir(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = source(
        &signin,
        &token,
        Some(path.clone()),
        Duration::from_millis(300),
    );
    let stem = crate::cache::iam_stem(
        &source.profile,
        &source.root_email,
        &source.username,
        source.signin.as_str().trim_end_matches('/'),
        source.token_url.as_str(),
    );
    std::fs::write(
        path.join(format!("{stem}.totp")),
        (SystemClock.utc_now().unix_timestamp() / 30 + 1).to_string(),
    )
    .unwrap();
    std::fs::set_permissions(
        path.join(format!("{stem}.totp")),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_err());
    assert_eq!(signin.requests(), 3);
    assert_eq!(token.requests(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_entry_is_token_cache_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let (signin, token) = peers(false);
    let path = dir.0.join("cache");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = source(&signin, &token, Some(path.clone()), Duration::from_secs(5));
    let stem = crate::cache::iam_stem(
        &source.profile,
        &source.root_email,
        &source.username,
        source.signin.as_str().trim_end_matches('/'),
        source.token_url.as_str(),
    );
    let entry = path.join(format!("{stem}.json"));
    std::fs::write(&entry, vec![b'x'; 1_048_577]).unwrap();
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (ctx, _) = RequestContext::new();
    assert_eq!(
        source.token(&ctx).await.err().unwrap().class(),
        ErrorClass::TokenCache
    );
    assert_eq!(signin.requests(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_during_signin_does_not_write_token_and_retry_works() {
    let dir = Dir::new();
    let (sent, started) = tokio::sync::oneshot::channel();
    let page = "<input name=\"_csrf\" value=\"csrf\">";
    let signin = Server::start(vec![
        Response::StallBeforeHead(sent),
        response("200 OK", None, page),
        response(
            "302 Found",
            Some("https://dashboard.test/return?code=toy"),
            "",
        ),
    ]);
    let token = Server::start(vec![response(
        "200 OK",
        None,
        r#"{"accessToken":"toy-token","expiresIn":3600}"#,
    )]);
    let path = dir.0.join("cache");
    let source = Arc::new(source(
        &signin,
        &token,
        Some(path.clone()),
        Duration::from_secs(5),
    ));
    let (ctx, cancel) = RequestContext::new();
    let task_source = source.clone();
    let task = tokio::spawn(async move { task_source.token(&ctx).await });
    started.await.unwrap();
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(std::fs::read_dir(&path).unwrap().all(|entry| {
        !entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|ext| ext == "json" || ext == "totp")
    }));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(token.requests(), 1);
}

#[tokio::test]
async fn external_totp_provider_is_used() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (signin, token) = peers(true);
    let mut source = source(&signin, &token, None, Duration::from_secs(5));
    source.seed = None;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    source.provider = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Secret::new("123456")) })
    }));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(signin.requests(), 4);
}

#[tokio::test]
async fn rejected_login_is_cooled_down() {
    let page = "<input name=\"_csrf\" value=\"csrf\">";
    let signin = Server::start(vec![
        response("200 OK", None, page),
        response("200 OK", None, page),
    ]);
    let token = Server::start(vec![]);
    let source = source(&signin, &token, None, Duration::from_secs(5));
    let (ctx, _) = RequestContext::new();
    let first = source.token(&ctx).await.err().unwrap();
    let second = source.token(&ctx).await.err().unwrap();
    assert_eq!(
        first.login_error().unwrap().reason(),
        second.login_error().unwrap().reason()
    );
    assert_eq!(signin.requests(), 2);
}

#[tokio::test]
async fn secrets_are_redacted_from_debug_and_errors() {
    let signin = Server::start(vec![response(
        "500 Error",
        None,
        "password-canary seed-canary token-canary",
    )]);
    let token = Server::start(vec![]);
    let mut source = source(&signin, &token, None, Duration::from_secs(5));
    source.password = Secret::new("password-canary");
    source.seed = Some(Secret::new("seed-canary"));
    let (ctx, _) = RequestContext::new();
    let error = source.token(&ctx).await.err().unwrap();
    let text = format!("{source:?} {error:?} {error}");
    for canary in ["password-canary", "seed-canary", "token-canary"] {
        assert!(!text.contains(canary));
    }
}
