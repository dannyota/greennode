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
    loop {
        if let Some(end) = context.check(clock) {
            return Err(Box::new(end.into()));
        }
        let worker_store = store.clone();
        let worker_name = name.clone();
        let worker = tokio::task::spawn_blocking(move || try_once(&worker_store, &worker_name));
        tokio::pin!(worker);
        let ended = context.wait(clock);
        tokio::pin!(ended);
        let result = poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(Box::new(end.into())));
            }
            match worker.as_mut().poll(cx) {
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
        let result = result?;
        if let Some(file) = result {
            return Ok(file);
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

fn try_once(store: &Store, name: &str) -> Result<Option<File>, Box<crate::Error>> {
    let file = store.open_lock(name)?;
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            store.verify_identity(name, &file)?;
            Ok(Some(file))
        }
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(_) => Err(Box::new(crate::Error::new(crate::ErrorClass::TokenCache))),
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
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
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
