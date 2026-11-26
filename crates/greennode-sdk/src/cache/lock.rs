use super::store::Store;
use std::fs::File;
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

#[allow(dead_code)]
pub(crate) async fn acquire(
    store: Arc<Store>,
    name: String,
    context: &crate::RequestContext,
    clock: &dyn crate::Clock,
) -> Result<File, Box<crate::Error>> {
    acquire_with(store, name, context, clock, || {}).await
}

async fn acquire_with(
    store: Arc<Store>,
    name: String,
    context: &crate::RequestContext,
    clock: &dyn crate::Clock,
    on_wait: impl Fn(),
) -> Result<File, Box<crate::Error>> {
    let mut delay = Duration::from_millis(10);
    let mut retained = None;
    loop {
        if let Some(end) = context.check(clock) {
            return Err(Box::new(end.into()));
        }
        let worker_store = store.clone();
        let worker_name = name.clone();
        let descriptor = retained.take();
        let mut worker = AbortOnDrop(tokio::task::spawn_blocking(move || {
            attempt(&worker_store, &worker_name, descriptor)
        }));
        let ended = context.wait(clock);
        tokio::pin!(ended);
        let result = poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(Box::new(end.into())));
            }
            match std::pin::Pin::new(&mut worker.0).poll(cx) {
                Poll::Ready(Ok(result)) => Poll::Ready(result),
                Poll::Ready(Err(_)) => Poll::Ready(Err(Box::new(crate::Error::new(
                    crate::ErrorClass::TokenCache,
                )))),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        if let Some(end) = context.check(clock) {
            return Err(Box::new(end.into()));
        }
        match result? {
            Attempt::Acquired(file) => return Ok(file),
            Attempt::Busy(file) => retained = Some(file),
            Attempt::Changed => {
                return Err(Box::new(crate::Error::new(crate::ErrorClass::TokenCache)));
            }
        }
        let wake = clock.monotonic_now() + delay;
        on_wait();
        let sleep = clock.sleep_until(wake);
        tokio::pin!(sleep);
        let ended = context.wait(clock);
        tokio::pin!(ended);
        poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(Box::new(end.into())));
            }
            if sleep.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Ok(()));
            }
            Poll::Pending
        })
        .await?;
        delay = (delay * 2).min(Duration::from_millis(500));
    }
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum Attempt {
    Busy(File),
    Acquired(File),
    Changed,
}

fn attempt(
    store: &Store,
    name: &str,
    retained: Option<File>,
) -> Result<Attempt, Box<crate::Error>> {
    let file = match retained {
        Some(file) => file,
        None => store.open_lock(name)?,
    };
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            if store.identity_matches(name, &file)? {
                Ok(Attempt::Acquired(file))
            } else {
                Ok(Attempt::Changed)
            }
        }
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(Attempt::Busy(file)),
        Err(_) => Err(Box::new(crate::Error::new(crate::ErrorClass::TokenCache))),
    }
}

#[cfg(test)]
fn try_once(store: &Store, name: &str) -> Result<Option<File>, Box<crate::Error>> {
    match attempt(store, name, None)? {
        Attempt::Acquired(file) => Ok(Some(file)),
        Attempt::Busy(_) | Attempt::Changed => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "greennode-lock-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir(&path).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn cancelled_queued_worker_does_not_open_lock() {
        let dir = Dir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        runtime.block_on(async {
            let (context, cancel) = crate::RequestContext::new();
            let (polled_tx, polled_rx) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                let clock = crate::SystemClock;
                let mut future = Box::pin(acquire(store, "entry.lock".into(), &context, &clock));
                let mut polled = Some(polled_tx);
                poll_fn(|cx| {
                    let result = future.as_mut().poll(cx);
                    if let Some(polled) = polled.take() {
                        let _ = polled.send(());
                    }
                    result
                })
                .await
            });
            polled_rx.await.unwrap();
            cancel.cancel();
            assert_eq!(
                task.await.unwrap().unwrap_err().class(),
                crate::ErrorClass::Cancelled
            );
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            tokio::task::spawn_blocking(|| {}).await.unwrap();
            assert!(!dir.0.join("entry.lock").exists());
        });
    }

    #[tokio::test]
    async fn waiter_keeps_descriptor_when_lock_name_is_replaced() {
        let dir = Dir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let first = try_once(&store, "entry.lock").unwrap().unwrap();
        let (context, _) = crate::RequestContext::new();
        let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let worker_clock = clock.clone();
        let ready = Arc::new(tokio::sync::Notify::new());
        let worker_ready = ready.clone();
        let worker_store = store.clone();
        let mut worker = tokio::spawn(async move {
            acquire_with(
                worker_store,
                "entry.lock".into(),
                &context,
                &worker_clock,
                || worker_ready.notify_one(),
            )
            .await
        });
        ready.notified().await;
        std::fs::rename(dir.0.join("entry.lock"), dir.0.join("old.lock")).unwrap();
        let replacement = store.open_lock("entry.lock").unwrap();
        drop(replacement);
        clock.advance(Duration::from_millis(10));
        let still_waiting = tokio::select! {
            _ = ready.notified() => true,
            _ = &mut worker => false,
        };
        assert!(still_waiting);
        drop(first);
        clock.advance(Duration::from_millis(20));
        let result = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(error) if error.class() == crate::ErrorClass::TokenCache));
        assert!(try_once(&store, "entry.lock").unwrap().is_some());
    }

    #[tokio::test]
    async fn waiting_writer_acquires_after_release() {
        let dir = Dir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let first = try_once(&store, "entry.lock").unwrap().unwrap();
        let (context, _) = crate::RequestContext::new();
        let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let worker_clock = clock.clone();
        let ready = Arc::new(tokio::sync::Notify::new());
        let worker_ready = ready.clone();
        let worker = tokio::spawn(async move {
            acquire_with(store, "entry.lock".into(), &context, &worker_clock, || {
                worker_ready.notify_one()
            })
            .await
        });
        ready.notified().await;
        assert!(!worker.is_finished());
        drop(first);
        clock.advance(Duration::from_millis(10));
        assert!(
            tokio::time::timeout(Duration::from_secs(2), worker)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn cancellation_while_waiting_returns_promptly() {
        let dir = Dir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let held = try_once(&store, "entry.lock").unwrap().unwrap();
        let (context, cancel) = crate::RequestContext::new();
        let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let ready = Arc::new(tokio::sync::Notify::new());
        let worker_ready = ready.clone();
        let worker = tokio::spawn(async move {
            acquire_with(store, "entry.lock".into(), &context, &clock, || {
                worker_ready.notify_one()
            })
            .await
        });
        ready.notified().await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().class(), crate::ErrorClass::Cancelled);
        drop(held);
    }

    #[tokio::test]
    async fn deadline_while_waiting_returns_promptly() {
        let dir = Dir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let held = try_once(&store, "entry.lock").unwrap().unwrap();
        let (parent, _) = crate::RequestContext::new();
        let clock = crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
        let deadline = crate::Clock::monotonic_now(&clock) + Duration::from_secs(1);
        let context = parent.child_with_deadline(deadline);
        let worker_clock = clock.clone();
        let ready = Arc::new(tokio::sync::Notify::new());
        let worker_ready = ready.clone();
        let worker = tokio::spawn(async move {
            acquire_with(store, "entry.lock".into(), &context, &worker_clock, || {
                worker_ready.notify_one()
            })
            .await
        });
        ready.notified().await;
        clock.advance(Duration::from_secs(1));
        let result = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.unwrap_err().class(),
            crate::ErrorClass::DeadlineExceeded
        );
        drop(held);
    }

    #[test]
    fn replaced_lock_is_not_same_file() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let original = store.open_lock("entry.lock").unwrap();
        assert!(store.verify_identity("entry.lock", &original).is_ok());
        std::fs::rename(dir.0.join("entry.lock"), dir.0.join("old.lock")).unwrap();
        let _replacement = store.open_lock("entry.lock").unwrap();
        assert!(store.verify_identity("entry.lock", &original).is_err());
    }

    #[test]
    fn lock_serializes_and_old_file_is_reused() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let first = try_once(&store, "entry.lock").unwrap().unwrap();
        assert!(try_once(&store, "entry.lock").unwrap().is_none());
        drop(first);
        assert!(dir.0.join("entry.lock").exists());
        assert!(try_once(&store, "entry.lock").unwrap().is_some());
    }
}
