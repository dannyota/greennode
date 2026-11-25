use super::*;
macro_rules! run {
    ($client:expr, $ctx:expr, $clock:expr, $deadline:expr, $op:expr, $auth:expr, $request:expr, $sleeper:expr, $jitter:expr) => {
        super::run(
            Runtime {
                client: $client,
                ctx: $ctx,
                clock: $clock,
                deadline: $deadline,
                sleeper: $sleeper,
                jitter: $jitter,
            },
            $op,
            $auth,
            $request,
        )
    };
}
use crate::wire::{
    connection::{HeadLimit, OneSendClient, PooledClient, Trust},
    test_server::{Response, Server},
};
use crate::{Clock, TestClock};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use time::OffsetDateTime;
mod evidence;
mod pooling;

#[derive(Default)]
struct Provider {
    count: AtomicUsize,
    rejected: Mutex<Vec<String>>,
}
impl CredentialsProvider for Provider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Token::new(
                Secret::new(if n == 0 { "toy-first" } else { "toy-next" }),
                None,
            ))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        rejected: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.rejected
                .lock()
                .unwrap()
                .push(rejected.expose_secret().to_owned());
            Ok(())
        })
    }
}
#[derive(Default)]
struct Immediate(AtomicUsize);
impl Sleeper for Immediate {
    fn sleep(&self, _: std::time::Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}
fn response(status: &str, headers: &[(&str, &str)]) -> Response {
    let mut headers: Vec<_> = headers
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    headers.push(("Content-Length".into(), "0".into()));
    Response::Http {
        status: status.into(),
        headers,
        body: Vec::new(),
    }
}
fn operation(method: Method, retry: redirect::Retry) -> Operation {
    Operation {
        method,
        class: redirect::Classification::Read,
        retry,
        no_redirect: false,
        allow_recovery: true,
        cap: 100,
        encoding: send::Encoding::Identity,
        retry_config: retry::Config {
            budget: 1,
            interval: std::time::Duration::from_secs(1),
        },
    }
}
fn request(uri: &str) -> Request {
    Request {
        url: Url::parse(uri).unwrap(),
        headers: HeaderMap::new(),
        body: Bytes::new(),
    }
}
fn client(server: &Server) -> Clients {
    Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::test(server.root_der.clone())).unwrap(),
    }
}
fn ctx() -> (RequestContext, TestClock, Instant) {
    let (ctx, _) = RequestContext::new();
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + std::time::Duration::from_secs(5);
    (ctx, clock, deadline)
}
#[test]
fn sender_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    let server = Server::start(vec![]);
    let (ctx, clock, deadline) = ctx();
    let client = client(&server);
    let op = operation(Method::GET, redirect::Retry::Repeatable);
    let request = request(&server.origin);
    let sleeper = Immediate::default();
    let mut jitter = || 0;
    let future = run!(
        &client,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &request,
        &sleeper,
        &mut jitter
    );
    assert_send(future);
}
#[tokio::test]
async fn status_retry() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(server.requests(), 2);
    assert_eq!(sleeper.0.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn once_no_retry() {
    let server = Server::start(vec![
        response("503 Service Unavailable", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Once),
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 503);
    assert_eq!(server.requests(), 1);
    assert_eq!(sleeper.0.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unauthorized_recovers() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(server.requests(), 2);
    assert_eq!(source.count.load(Ordering::SeqCst), 2);
    assert!(source.rejected.lock().unwrap().as_slice() == ["toy-first"]);
    let heads = server.heads();
    assert!(
        heads[0]
            .windows(b"authorization: Bearer toy-first".len())
            .any(|value| value.eq_ignore_ascii_case(b"authorization: Bearer toy-first"))
    );
    assert!(
        heads[1]
            .windows(b"authorization: Bearer toy-next".len())
            .any(|value| value.eq_ignore_ascii_case(b"authorization: Bearer toy-next"))
    );
}
#[tokio::test]
async fn unauthorized_recovery_gets_a_fresh_retry_budget() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("401 Unauthorized", &[]),
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(server.requests(), 4);
    assert_eq!(sleeper.0.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn second_unauthorized_stops() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        response("401 Unauthorized", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 401);
    assert_eq!(server.requests(), 2);
    assert!(source.rejected.lock().unwrap().as_slice() == ["toy-first"]);
}
#[tokio::test]
async fn same_origin_redirect() {
    let server = Server::start(vec![
        response("302 Found", &[("Location", "/next")]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 200);
    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads[1].starts_with(b"GET /next "));
    assert!(heads.iter().all(|h| {
        h.windows(b"authorization: Bearer toy-first".len())
            .any(|x| x.eq_ignore_ascii_case(b"authorization: Bearer toy-first"))
    }));
}
#[tokio::test]
async fn cross_origin_redirect_never_sends_credentials() {
    let other = Server::start(vec![response("200 OK", &[])]);
    let target = format!("{}/other", other.origin);
    let server = Server::start(vec![response("302 Found", &[("Location", &target)])]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Redirect(_),
            ..
        })
    ));
    assert_eq!(server.requests(), 1);
    assert_eq!(other.requests(), 0);
}
struct BuiltIn {
    obtained: Instant,
    rejected: Mutex<Vec<bool>>,
}
impl BearerSource for BuiltIn {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(BearerToken {
                token: Token::new(Secret::new("toy-first"), None),
                obtained_at: Some(unauthorized::ObtainedAt::InProcess(self.obtained)),
            })
        })
    }
    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async move {
            assert!(sent.expose_secret() == "toy-first");
            self.rejected.lock().unwrap().push(evict);
            Ok(())
        })
    }
}
#[tokio::test]
async fn young_builtin_rejection_is_recorded_without_eviction() {
    let server = Server::start(vec![response("401 Unauthorized", &[])]);
    let (ctx, clock, deadline) = ctx();
    let source = BuiltIn {
        obtained: clock.monotonic_now(),
        rejected: Mutex::new(Vec::new()),
    };
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Once),
        Auth::Bearer(&source),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 401);
    assert!(source.rejected.lock().unwrap().as_slice() == [false]);
    assert_eq!(server.requests(), 1);
}
#[tokio::test]
async fn once_401_invalidates_without_resend() {
    let server = Server::start(vec![
        response("401 Unauthorized", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let reply = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Once),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 401);
    assert_eq!(server.requests(), 1);
    assert_eq!(source.count.load(Ordering::SeqCst), 1);
    assert!(source.rejected.lock().unwrap().as_slice() == ["toy-first"]);
}
#[tokio::test]
async fn post_head_401_invalidates_without_resend() {
    let server = Server::start(vec![
        Response::RawClose(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 8\r\n\r\nx".to_vec()),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Send(send::Error::AfterHead { .. }),
            ..
        })
    ));
    assert_eq!(server.requests(), 1);
    assert!(source.rejected.lock().unwrap().as_slice() == ["toy-first"]);
}
struct UnsafeProvider;
impl CredentialsProvider for UnsafeProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(Token::new(Secret::new("bad token"), None)) })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
struct CalledSource(AtomicUsize);
impl BearerSource for CalledSource {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, crate::Error>> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
    fn rejected<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
        _: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
#[tokio::test]
async fn cancelled_context_never_invokes_source() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let (ctx, cancel) = RequestContext::new();
    cancel.cancel();
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + std::time::Duration::from_secs(5);
    let source = CalledSource(AtomicUsize::new(0));
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&source),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Context(crate::ContextEnd::Cancelled),
            ..
        })
    ));
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
    assert_eq!(server.requests(), 0);
}
#[tokio::test]
async fn invalid_endpoint_is_refused_before_credentials() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let (ctx, clock, deadline) = ctx();
    let source = Provider::default();
    let sleeper = Immediate::default();
    let invalid = request(&format!("{}/bad%zz", server.origin));
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &invalid,
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::InvalidRequest,
            ..
        })
    ));
    assert_eq!(source.count.load(Ordering::SeqCst), 0);
    assert_eq!(server.requests(), 0);
}
#[tokio::test]
async fn unsafe_bearer_is_never_sent() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let (ctx, clock, deadline) = ctx();
    let source = UnsafeProvider;
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::GET, redirect::Retry::Repeatable),
        Auth::Bearer(&PublicSource(&source)),
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::InvalidRequest,
            ..
        })
    ));
    assert_eq!(server.requests(), 0);
}
#[tokio::test]
async fn post_redirect_refused() {
    let server = Server::start(vec![
        response("302 Found", &[("Location", "/next")]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let result = run!(
        &client(&server),
        &ctx,
        &clock,
        deadline,
        &operation(Method::POST, redirect::Retry::Repeatable),
        Auth::None,
        &request(&server.origin),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Redirect(_),
            ..
        })
    ));
    assert_eq!(server.requests(), 1);
}
#[tokio::test]
async fn failed_dial_retries_nonidempotent() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::webpki()).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::webpki()).unwrap(),
    };
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let result = run!(
        &client,
        &ctx,
        &clock,
        deadline,
        &operation(Method::POST, redirect::Retry::NonIdempotent),
        Auth::None,
        &request(&format!("https://{address}/")),
        &sleeper,
        &mut || 0
    )
    .await;
    assert!(matches!(
        result,
        Err(Failure {
            error: Error::Send(_),
            ..
        })
    ));
    assert_eq!(sleeper.0.load(Ordering::SeqCst), 1);
}
struct Blocking {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Notify,
    completed: AtomicBool,
}
impl Sleeper for Blocking {
    fn sleep(&self, _: std::time::Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        if let Some(sender) = self.started.lock().unwrap().take() {
            let _ = sender.send(());
        }
        Box::pin(async move {
            self.release.notified().await;
            self.completed.store(true, Ordering::SeqCst);
        })
    }
}
#[tokio::test]
async fn deadline_stops_retry_sleep() {
    let server = Server::start(vec![
        response("429 Too Many Requests", &[]),
        response("200 OK", &[]),
    ]);
    let (ctx, clock, deadline) = ctx();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sleeper = Blocking {
        started: Mutex::new(Some(tx)),
        release: tokio::sync::Notify::new(),
        completed: AtomicBool::new(false),
    };
    let client = client(&server);
    let op = operation(Method::GET, redirect::Retry::Repeatable);
    let req = request(&server.origin);
    let mut jitter = || 0;
    let future = run!(
        &client,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &req,
        &sleeper,
        &mut jitter
    );
    tokio::pin!(future);
    tokio::select! { _=&mut future => panic!("ended before sleep"), value=rx => assert!(value.is_ok()) }
    clock.advance(std::time::Duration::from_secs(5));
    sleeper.release.notify_one();
    let failure = match future.await {
        Err(error) => error,
        Ok(_) => panic!("expected deadline"),
    };
    assert!(matches!(
        failure.error,
        Error::Context(crate::ContextEnd::DeadlineExceeded)
    ));
    assert_eq!(failure.status, Some(http::StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(failure.dispatch, crate::WriteDispatch::NotAWrite);
    assert_eq!(server.requests(), 1);
    assert!(!sleeper.completed.load(Ordering::SeqCst));
}
