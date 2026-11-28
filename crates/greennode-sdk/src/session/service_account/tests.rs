use super::*;
use crate::wire::test_server::{Response, Server};
use std::{env, path::PathBuf};

fn server(count: usize) -> Server {
    let responses = (0..count)
        .map(|_| {
            let body = br#"{"access_token":"canary-token","expires_in":120}"#.to_vec();
            Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body,
            }
        })
        .collect();
    Server::start(responses)
}

fn source(
    server: &Server,
    clock: Arc<crate::TestClock>,
    directory: Option<PathBuf>,
) -> ServiceAccount {
    let mut source = ServiceAccount::new(
        "client".into(),
        Secret::new("canary-secret"),
        "default".into(),
        directory,
        None,
        clock,
        Duration::ZERO,
    );
    source.endpoint = Some((server.origin.clone(), server.root_der.clone()));
    source
}

#[cfg(unix)]
struct Dir(PathBuf);
#[cfg(unix)]
impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-session-{}-{:?}",
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

#[tokio::test]
async fn cold_start_obtains_a_token() {
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock, None);
    let (context, _) = RequestContext::new();
    assert!(source.token(&context).await.is_ok());
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn disk_cache_is_reused_by_another_source() {
    let dir = Dir::new();
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let first = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let second = source(&server, clock, Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    assert!(first.token(&ctx).await.is_ok());
    let task = tokio::spawn(async move { second.token(&ctx).await.is_ok() });
    assert!(task.await.unwrap());
    assert_eq!(server.requests(), 1);
    let entries = std::fs::read_dir(dir.0.join("cache/service-account-v1"))
        .unwrap()
        .count();
    assert_eq!(entries, 2);
}

#[tokio::test]
async fn concurrent_callers_share_exchange() {
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = Arc::new(source(&server, clock, None));
    let (ctx, _) = RequestContext::new();
    let a = source.clone();
    let b = source.clone();
    let other = ctx.clone();
    let first = tokio::spawn(async move { a.token(&ctx).await.is_ok() });
    let second = tokio::spawn(async move { b.token(&other).await.is_ok() });
    assert!(first.await.unwrap());
    assert!(second.await.unwrap());
    assert_eq!(server.requests(), 1);
}

#[test]
fn in_memory_expiry_uses_monotonic_time() {
    use std::sync::atomic::{AtomicI64, Ordering};
    struct JumpClock {
        clock: crate::TestClock,
        jump: AtomicI64,
    }
    impl Clock for JumpClock {
        fn utc_now(&self) -> time::OffsetDateTime {
            self.clock.utc_now() + time::Duration::seconds(self.jump.load(Ordering::SeqCst))
        }
        fn monotonic_now(&self) -> tokio::time::Instant {
            self.clock.monotonic_now()
        }
        fn sleep_until(
            &self,
            deadline: tokio::time::Instant,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.clock.sleep_until(deadline)
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = server(2);
        let clock = Arc::new(JumpClock {
            clock: crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
            jump: AtomicI64::new(0),
        });
        let mut source = ServiceAccount::new(
            "client".into(),
            Secret::new("canary-secret"),
            "default".into(),
            None,
            None,
            clock.clone(),
            Duration::ZERO,
        );
        source.endpoint = Some((server.origin.clone(), server.root_der.clone()));
        let (ctx, _) = RequestContext::new();
        assert!(source.token(&ctx).await.is_ok());
        clock.jump.store(300, Ordering::SeqCst);
        assert!(source.token(&ctx).await.is_ok());
        assert_eq!(server.requests(), 1);
        clock.jump.store(-300, Ordering::SeqCst);
        clock.clock.advance(Duration::from_secs(91));
        assert!(source.token(&ctx).await.is_ok());
        assert_eq!(server.requests(), 2);
    });
}

#[tokio::test]
async fn expiry_uses_exchange_clock_sample() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct JumpClock {
        clock: crate::TestClock,
        samples: AtomicUsize,
    }
    impl Clock for JumpClock {
        fn utc_now(&self) -> time::OffsetDateTime {
            let jump = if self.samples.fetch_add(1, Ordering::SeqCst) == 0 {
                0
            } else {
                300
            };
            self.clock.utc_now() + time::Duration::seconds(jump)
        }
        fn monotonic_now(&self) -> tokio::time::Instant {
            self.clock.monotonic_now()
        }
        fn sleep_until(
            &self,
            deadline: tokio::time::Instant,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.clock.sleep_until(deadline)
        }
    }
    let server = server(2);
    let clock = Arc::new(JumpClock {
        clock: crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
        samples: AtomicUsize::new(0),
    });
    let mut source = ServiceAccount::new(
        "client".into(),
        Secret::new("canary-secret"),
        "default".into(),
        None,
        None,
        clock.clone(),
        Duration::ZERO,
    );
    source.endpoint = Some((server.origin.clone(), server.root_der.clone()));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    clock.clock.advance(Duration::from_secs(91));
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn expired_token_refreshes() {
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock.clone(), None);
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    clock.advance(Duration::from_secs(91));
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn rejection_checks_age_and_exact_token() {
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock.clone(), None);
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert!(
        source
            .rejected(&ctx, &Secret::new("canary-token"), true)
            .await
            .is_ok()
    );
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 1);
    clock.advance(Duration::from_secs(30));
    assert!(
        source
            .rejected(&ctx, &Secret::new("different"), true)
            .await
            .is_ok()
    );
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 1);
    assert!(
        source
            .rejected(&ctx, &Secret::new("canary-token"), true)
            .await
            .is_ok()
    );
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn disk_entry_expiring_during_read_is_refreshed() {
    let dir = Dir::new();
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let first = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    let initial = first.token(&ctx).await;
    assert!(initial.is_ok(), "initial error: {:?}", initial.err());
    let mut second = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = std::sync::Mutex::new(Some(started_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    second.read_hook = Some(Arc::new(move || {
        let _ = started_tx.lock().unwrap().take().unwrap().send(());
        release_rx.lock().unwrap().recv().unwrap();
    }));
    let task = tokio::spawn(async move { second.token(&ctx).await.is_ok() });
    started_rx.await.unwrap();
    clock.advance(Duration::from_secs(91));
    release_tx.send(()).unwrap();
    assert!(task.await.unwrap());
    assert_eq!(server.requests(), 2);
}

#[cfg(unix)]
#[test]
fn dropped_refresh_does_not_publish_and_releases_lock() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let dir = Dir::new();
        let server = server(2);
        let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
        let mut first = source(&server, clock.clone(), Some(dir.0.join("cache")));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let started_tx = std::sync::Mutex::new(Some(started_tx));
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        first.sync_hook = Some(Arc::new(move || {
            let _ = started_tx.lock().unwrap().take().unwrap().send(());
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let (ctx, _) = RequestContext::new();
        let task = tokio::spawn(async move { first.token(&ctx).await.map(|_| ()) });
        started_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        let second = source(&server, clock, Some(dir.0.join("cache")));
        let (ctx, _) = RequestContext::new();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), second.token(&ctx))
                .await
                .unwrap()
                .is_ok()
        );
        assert_eq!(server.requests(), 2);
    });
}

#[cfg(unix)]
#[test]
fn drop_during_stalled_rename_returns_promptly() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = Dir::new();
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(crate::cache::store::Store::open(&dir.0).unwrap());
    let live = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drop_guard = LiveWrite(live.clone());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let calls = AtomicUsize::new(0);
    let worker = std::thread::spawn(move || {
        let entry = crate::cache::entry::Entry {
            token: Secret::new("toy"),
            expires: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(120),
            obtained: time::OffsetDateTime::UNIX_EPOCH,
        };
        crate::cache::entry::write_hook(
            &store,
            "entry.json",
            &entry,
            || {
                if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                Ok(())
            },
            None,
            Some(&live),
        )
    });
    started_rx.recv_timeout(Duration::from_secs(60)).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(drop_guard);
        done_tx.send(()).unwrap();
    });
    let prompt = done_rx.recv_timeout(Duration::from_secs(60)).is_ok();
    release_tx.send(()).unwrap();
    dropper.join().unwrap();
    let result = worker.join().unwrap();
    assert!(prompt);
    assert!(result.is_err());
    assert!(!dir.0.join("entry.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn token_expiring_during_write_returns_auth_error_without_second_exchange() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = Dir::new();
    let body = br#"{"access_token":"canary-token","expires_in":31}"#.to_vec();
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }]);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let mut account = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = std::sync::Mutex::new(Some(started_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let first = AtomicBool::new(true);
    account.sync_hook = Some(Arc::new(move || {
        if first.swap(false, Ordering::SeqCst) {
            let _ = started_tx.lock().unwrap().take().unwrap().send(());
            release_rx.lock().unwrap().recv().unwrap();
        }
    }));
    let (ctx, _) = RequestContext::new();
    let task = tokio::spawn(async move { account.token(&ctx).await.map(|_| ()) });
    started_rx.await.unwrap();
    clock.advance(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    assert!(matches!(task.await.unwrap(), Err(error) if error.class() == ErrorClass::Auth));
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn unsafe_cache_entry_returns_cache_error() {
    let dir = Dir::new();
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let first = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    assert!(first.token(&ctx).await.is_ok());
    let namespace = dir.0.join("cache/service-account-v1");
    let stem = crate::cache::service_account_stem(
        "default",
        "client",
        "https://iamapis.vngcloud.vn/accounts-api/v2/auth/token",
        &Secret::new("canary-secret"),
    );
    let entry = namespace.join(format!("{stem}.json"));
    std::fs::remove_file(&entry).unwrap();
    std::os::unix::fs::symlink("missing", &entry).unwrap();
    let second = source(&server, clock, Some(dir.0.join("cache")));
    assert!(matches!(second.token(&ctx).await, Err(e) if e.class() == ErrorClass::TokenCache));
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn write_failure_keeps_exchanged_token_in_memory() {
    let dir = Dir::new();
    let body = br#"{"access_token":"token<unsafe-for-cache","expires_in":120}"#.to_vec();
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }]);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock, Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_disk_token_remains_pending_after_cancellation() {
    let dir = Dir::new();
    let server = server(2);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    clock.advance(Duration::from_secs(30));
    assert!(
        source
            .rejected(&ctx, &Secret::new("canary-token"), true)
            .await
            .is_ok()
    );
    let (cancelled, cancel) = RequestContext::new();
    cancel.cancel();
    assert!(matches!(source.token(&cancelled).await, Err(e) if e.class() == ErrorClass::Cancelled));
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn unsafe_cache_lock_returns_cache_error() {
    let dir = Dir::new();
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let path = dir.0.join("cache/service-account-v1");
    std::fs::create_dir_all(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.0.join("cache"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let stem = crate::cache::service_account_stem(
        "default",
        "client",
        "https://iamapis.vngcloud.vn/accounts-api/v2/auth/token",
        &Secret::new("canary-secret"),
    );
    std::os::unix::fs::symlink("missing", path.join(format!("{stem}.lock"))).unwrap();
    let source = source(&server, clock, Some(dir.0.join("cache")));
    let (ctx, _) = RequestContext::new();
    assert!(matches!(source.token(&ctx).await, Err(e) if e.class() == ErrorClass::TokenCache));
    assert_eq!(server.requests(), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_uses_only_memory() {
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let path = env::temp_dir().join(format!("greennode-session-{}", std::process::id()));
    let source = source(&server, clock, Some(path.clone()));
    let (ctx, _) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    assert!(source.token(&ctx).await.is_ok());
    assert_eq!(server.requests(), 1);
    assert!(!path.exists());
}

#[cfg(unix)]
async fn ended_exchange_recovers(deadline: bool) {
    let dir = Dir::new();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let body = br#"{"access_token":"canary-token","expires_in":120}"#.to_vec();
    let server = Server::start(vec![
        Response::StallBeforeHead(started_tx),
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body,
        },
    ]);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = Arc::new(source(&server, clock.clone(), Some(dir.0.join("cache"))));
    let (parent, cancel) = RequestContext::new();
    let ctx = if deadline {
        parent.child_with_deadline(clock.monotonic_now() + Duration::from_secs(1))
    } else {
        parent.clone()
    };
    let worker = source.clone();
    let task = tokio::spawn(async move { worker.token(&ctx).await.map(|_| ()) });
    started_rx.await.unwrap();
    if deadline {
        clock.advance(Duration::from_secs(1));
    } else {
        cancel.cancel();
    }
    let result = tokio::time::timeout(Duration::from_secs(60), task).await;
    assert!(result.is_ok());
    let result = result.unwrap().unwrap();
    let class = if deadline {
        ErrorClass::DeadlineExceeded
    } else {
        ErrorClass::Cancelled
    };
    assert!(matches!(result, Err(error) if error.class() == class));
    let namespace = dir.0.join("cache/service-account-v1");
    assert!(std::fs::read_dir(namespace).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|ext| ext != "json")
    }));
    let (fresh, _) = RequestContext::new();
    assert!(source.token(&fresh).await.is_ok());
    assert_eq!(server.requests(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_during_exchange_does_not_publish() {
    ended_exchange_recovers(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_during_exchange_does_not_publish() {
    ended_exchange_recovers(true).await;
}

#[cfg(unix)]
fn ended_disk_worker_recovers(deadline: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = Dir::new();
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let mut account = source(&server, clock.clone(), Some(dir.0.join("cache")));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = std::sync::Mutex::new(Some(started_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let first = AtomicBool::new(true);
    account.disk_hook = Some(Arc::new(move || {
        if first.swap(false, Ordering::SeqCst) {
            let _ = started_tx.lock().unwrap().take().unwrap().send(());
            release_rx.lock().unwrap().recv().unwrap();
        }
    }));
    let source = Arc::new(account);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (parent, cancel) = RequestContext::new();
        let ctx = if deadline {
            parent.child_with_deadline(clock.monotonic_now() + Duration::from_secs(1))
        } else {
            parent.clone()
        };
        let worker = source.clone();
        let task = tokio::spawn(async move { worker.token(&ctx).await.map(|_| ()) });
        started_rx.await.unwrap();
        if deadline {
            clock.advance(Duration::from_secs(1));
        } else {
            cancel.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(60), task).await;
        release_tx.send(()).unwrap();
        tokio::task::spawn_blocking(|| {}).await.unwrap();
        let class = if deadline {
            ErrorClass::DeadlineExceeded
        } else {
            ErrorClass::Cancelled
        };
        assert!(matches!(result.unwrap().unwrap(), Err(error) if error.class() == class));
        assert!(!dir.0.join("cache").exists());
        assert_eq!(server.requests(), 0);
        let (fresh, _) = RequestContext::new();
        assert!(source.token(&fresh).await.is_ok());
        assert_eq!(server.requests(), 1);
    });
}

#[cfg(unix)]
#[test]
fn cancellation_during_disk_worker_does_not_publish() {
    ended_disk_worker_recovers(false);
}

#[cfg(unix)]
#[test]
fn deadline_during_disk_worker_does_not_publish() {
    ended_disk_worker_recovers(true);
}

#[tokio::test]
async fn cancelled_context_does_not_exchange() {
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock, None);
    let (ctx, cancel) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    cancel.cancel();
    assert!(matches!(source.token(&ctx).await, Err(e) if e.class() == ErrorClass::Cancelled));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn debug_and_errors_redact_secrets() {
    let server = server(1);
    let clock = Arc::new(crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let source = source(&server, clock, None);
    let (ctx, cancel) = RequestContext::new();
    assert!(source.token(&ctx).await.is_ok());
    let debug = format!("{source:?}");
    assert!(!debug.contains("canary-token") && !debug.contains("canary-secret"));
    cancel.cancel();
    let error = source.token(&ctx).await.err().unwrap();
    let text = format!("{error:?} {error}");
    assert!(!text.contains("canary-token") && !text.contains("canary-secret"));
}
