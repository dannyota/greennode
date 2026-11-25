use super::*;
use crate::{Error as SdkError, WriteDispatch};
use http::StatusCode;

fn write(method: Method, retry: redirect::Retry) -> Operation {
    let mut op = operation(method, retry);
    op.class = redirect::Classification::Write;
    op
}
fn failure(result: Result<super::super::Response, Failure>) -> Failure {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected failure"),
    }
}
#[test]
fn provider_error_text_is_not_exposed() {
    use crate::error::{ApiError, SafeText};
    let raw = SdkError::from_api(ApiError::new(
        "toy-provider",
        Some(401),
        SafeText::new("", &[]),
        SafeText::new("toy-provider-canary", &[]),
        false,
    ));
    let failure = Failure {
        error: Error::Provider,
        status: Some(StatusCode::UNAUTHORIZED),
        dispatch: WriteDispatch::MayHaveBeenSent,
        credentials: SentCredentials::default(),
    };
    let sdk: crate::Failure<()> = failure.into_failure(raw);
    assert!(sdk.error().api_error().is_none());
    assert!(!format!("{:?}", sdk.error()).contains("toy-provider-canary"));
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(sdk.error());
    while let Some(error) = source {
        assert!(!format!("{error:?}").contains("toy-provider-canary"));
        assert!(!error.to_string().contains("toy-provider-canary"));
        source = error.source();
    }
}
#[test]
fn status_detail_preserves_identity_and_api_error() {
    use crate::error::{ApiError, SafeText};
    let mapped = SdkError::from_api(ApiError::new(
        "toy-operation",
        Some(401),
        SafeText::new("", &[]),
        SafeText::new("", &[]),
        false,
    ));
    assert!(!mapped.has_identity(crate::ErrorClass::Auth));
    let before = mapped.api_error().map(ApiError::operation);
    let failure = Failure {
        error: Error::InvalidRequest,
        status: Some(StatusCode::UNAUTHORIZED),
        dispatch: WriteDispatch::NotSent,
        credentials: SentCredentials::default(),
    };
    let sdk: crate::Failure<()> = failure.into_failure(mapped);
    assert!(!sdk.error().has_identity(crate::ErrorClass::Auth));
    assert_eq!(sdk.error().api_error().map(ApiError::operation), before);
    assert_eq!(
        sdk.error().detail::<StatusCode>(),
        Some(&StatusCode::UNAUTHORIZED)
    );
    let explicit = SdkError::new(crate::ErrorClass::Auth)
        .with_detail(17u32)
        .with_outer_detail(StatusCode::UNAUTHORIZED);
    assert!(explicit.has_identity(crate::ErrorClass::Auth));
    assert_eq!(explicit.detail::<u32>(), Some(&17));
    assert!(explicit.api_error().is_none());
}
#[tokio::test]
async fn cancel_before_send_stays_not_sent() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, cancel) = RequestContext::new();
    cancel.cancel();
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + std::time::Duration::from_secs(5);
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::None,
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert_eq!(err.dispatch, WriteDispatch::NotSent);
    let sdk: crate::Failure<()> = err.into_failure(SdkError::provider_failed());
    assert_eq!(sdk.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}
struct BeforeRequestClock {
    start: Instant,
    sleeps: AtomicUsize,
}
impl Clock for BeforeRequestClock {
    fn utc_now(&self) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH
    }
    fn monotonic_now(&self) -> Instant {
        if self.sleeps.load(Ordering::SeqCst) >= 2 {
            self.start + std::time::Duration::from_secs(5)
        } else {
            self.start
        }
    }
    fn sleep_until(&self, _: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn deadline_before_request_poll_stays_not_sent() {
    let server = Server::start(vec![response("200 OK", &[])]);
    let clients = client(&server);
    let (ctx, _) = RequestContext::new();
    let clock = BeforeRequestClock {
        start: Instant::now(),
        sleeps: AtomicUsize::new(0),
    };
    let deadline = clock.start + std::time::Duration::from_secs(5);
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::None,
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert_eq!(err.dispatch, WriteDispatch::NotSent);
    assert_eq!(server.requests(), 0);
}
struct HeadClock {
    clock: TestClock,
    calls: AtomicUsize,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl Clock for HeadClock {
    fn utc_now(&self) -> OffsetDateTime {
        self.clock.utc_now()
    }
    fn monotonic_now(&self) -> Instant {
        self.clock.monotonic_now()
    }
    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 2
            && let Some(sender) = self.entered.lock().unwrap().take()
        {
            let _ = sender.send(());
        }
        self.clock.sleep_until(deadline)
    }
}
#[tokio::test]
async fn cancel_during_body_retains_observed_status() {
    let (sent, _) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::Stall(sent, 9)]);
    let clients = client(&server);
    let (ctx, cancel) = RequestContext::new();
    let base = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = base.monotonic_now() + std::time::Duration::from_secs(5);
    let (started, received) = tokio::sync::oneshot::channel();
    let clock = HeadClock {
        clock: base,
        calls: AtomicUsize::new(0),
        entered: Mutex::new(Some(started)),
    };
    let sleeper = Immediate::default();
    let op = write(Method::POST, redirect::Retry::Once);
    let request = request(&server.origin);
    let mut jitter = || 0;
    let future = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &request,
        &sleeper,
        &mut jitter
    );
    tokio::pin!(future);
    tokio::select! { result=&mut future=>panic!("ended before cancellation: {}",result.is_ok()), ready=received=>assert!(ready.is_ok()) }
    cancel.cancel();
    let err = failure(future.await);
    assert_eq!(err.status, Some(StatusCode::OK));
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
}
#[tokio::test]
async fn cancel_waiting_for_headers_is_possibly_sent() {
    let (sent, received) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let clients = client(&server);
    let (ctx, cancel) = RequestContext::new();
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + std::time::Duration::from_secs(5);
    let sleeper = Immediate::default();
    let op = write(Method::POST, redirect::Retry::Once);
    let request = request(&server.origin);
    let mut jitter = || 0;
    let future = run!(
        &clients,
        &ctx,
        &clock,
        deadline,
        &op,
        Auth::None,
        &request,
        &sleeper,
        &mut jitter
    );
    tokio::pin!(future);
    tokio::select! { result=&mut future=>panic!("ended before cancellation: {}", result.is_ok()), ready=received=>assert!(ready.is_ok()) }
    cancel.cancel();
    let err = failure(future.await);
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    let sdk: crate::Failure<()> = err.into_failure(SdkError::provider_failed());
    assert_eq!(sdk.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(server.requests(), 1);
}
struct StopServer(Mutex<Option<Server>>);
impl Sleeper for StopServer {
    fn sleep(&self, _: std::time::Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        drop(self.0.lock().unwrap().take());
        Box::pin(async {})
    }
}
#[tokio::test]
async fn failed_first_dial_stays_not_sent() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let clients = Clients {
        pooled: PooledClient::new(HeadLimit::Api, Trust::webpki()).unwrap(),
        one_send: OneSendClient::new(HeadLimit::Api, Trust::webpki()).unwrap(),
    };
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::None,
            &request(&format!("https://{address}/")),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert!(matches!(
        err.error,
        Error::Send(send::Error::Transport(retry::SendFailure::FailedDial))
    ));
    assert_eq!(err.dispatch, WriteDispatch::NotSent);
    assert!(err.status.is_none());
}
#[tokio::test]
async fn failed_dial_after_response_keeps_dispatch_evidence() {
    let server = Server::start(vec![response("429 Too Many Requests", &[])]);
    let clients = client(&server);
    let uri = server.origin.clone();
    let sleeper = StopServer(Mutex::new(Some(server)));
    let (ctx, clock, deadline) = ctx();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::NonIdempotent),
            Auth::None,
            &request(&uri),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert!(matches!(
        err.error,
        Error::Send(send::Error::Transport(retry::SendFailure::FailedDial))
    ));
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    assert_eq!(err.status, None);
}
#[tokio::test]
async fn failure_after_401_recovery_keeps_dispatch_evidence() {
    let server = Server::start(vec![response("401 Unauthorized", &[]), Response::Close]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let source = Provider::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::NonIdempotent),
            Auth::Bearer(&PublicSource(&source)),
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    assert_eq!(source.count.load(Ordering::SeqCst), 2);
    assert_eq!(server.requests(), 2);
}
struct FailInvalidation;
impl CredentialsProvider for FailInvalidation {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, SdkError>> + Send + 'a>> {
        Box::pin(async { Ok(Token::new(Secret::new("toy"), None)) })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), SdkError>> + Send + 'a>> {
        Box::pin(async {
            use crate::error::{ApiError, SafeText};
            Err(SdkError::from_api(ApiError::new(
                "toy-provider",
                Some(401),
                SafeText::new("", &[]),
                SafeText::new("toy-provider-canary", &[]),
                false,
            )))
        })
    }
}
#[tokio::test]
async fn once_invalidation_error_keeps_status_and_dispatch() {
    let server = Server::start(vec![response(
        "401 Unauthorized",
        &[("X-Canary", "toy-header")],
    )]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::Bearer(&PublicSource(&FailInvalidation)),
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert!(matches!(err.error, Error::Provider));
    assert_eq!(err.status, Some(StatusCode::UNAUTHORIZED));
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(!format!("{err:?}").contains("toy-header"));
    assert!(!format!("{err:?}").contains("toy-provider-canary"));
    let sdk: crate::Failure<()> = err.into_failure(SdkError::provider_failed());
    assert!(sdk.error().api_error().is_none());
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(sdk.error());
    while let Some(error) = source {
        assert!(!format!("{error:?}").contains("toy-provider-canary"));
        assert!(!error.to_string().contains("toy-provider-canary"));
        source = error.source();
    }
    assert!(sdk.error().has_identity(crate::ErrorClass::Auth));
    assert_eq!(
        sdk.error().detail::<StatusCode>(),
        Some(&StatusCode::UNAUTHORIZED)
    );
    assert_eq!(sdk.write_dispatch(), WriteDispatch::MayHaveBeenSent);
}
#[tokio::test]
async fn refused_redirect_keeps_status() {
    let server = Server::start(vec![response("302 Found", &[("Location", "/toy-secret")])]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::None,
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert!(matches!(err.error, Error::Redirect(_)));
    assert_eq!(err.status, Some(StatusCode::FOUND));
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(!format!("{err:?}").contains("toy-secret"));
}
#[tokio::test]
async fn post_head_failure_keeps_status_without_headers() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 9\r\nX-Canary: toy-header\r\n\r\nx"
            .to_vec(),
    )]);
    let clients = client(&server);
    let (ctx, clock, deadline) = ctx();
    let sleeper = Immediate::default();
    let err = failure(
        run!(
            &clients,
            &ctx,
            &clock,
            deadline,
            &write(Method::POST, redirect::Retry::Once),
            Auth::None,
            &request(&server.origin),
            &sleeper,
            &mut || 0
        )
        .await,
    );
    assert_eq!(err.status, Some(StatusCode::SERVICE_UNAVAILABLE));
    assert_eq!(err.dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(!format!("{err:?}").contains("toy-header"));
}
