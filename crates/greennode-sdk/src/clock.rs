use std::{future::Future, pin::Pin, time::Duration};

use time::OffsetDateTime;
use tokio::{sync::watch, time::Instant};

/// Source of wall time, monotonic time, and deadline waits.
pub trait Clock: Send + Sync {
    /// Returns the current UTC wall time.
    fn utc_now(&self) -> OffsetDateTime;
    /// Returns an instant suitable for measuring elapsed time.
    fn monotonic_now(&self) -> Instant;
    /// Completes when the monotonic deadline is reached.
    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    fn response_body_started(&self) {}

    #[cfg(any(test, feature = "test-support"))]
    /// Lets test clocks interrupt settlement after a confirmation read completes.
    fn volume_confirmation_read_completed(&self) {}
}

/// Uses the system wall clock and Tokio monotonic clock.
#[derive(Clone, Copy, Default)]
pub struct SystemClock;

crate::error::debug_record!(SystemClock {});

impl Clock for SystemClock {
    fn utc_now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep_until(deadline))
    }
}

#[derive(Clone)]
struct TimeState {
    utc: OffsetDateTime,
    monotonic: Instant,
}

/// A manually advanced clock shared by its clones.
#[derive(Clone)]
pub struct TestClock {
    state: watch::Sender<TimeState>,
}

impl TestClock {
    /// Starts at the given wall time and the current Tokio instant.
    pub fn new(utc: OffsetDateTime) -> Self {
        let (state, _) = watch::channel(TimeState {
            utc,
            monotonic: Instant::now(),
        });
        Self { state }
    }

    /// Advances both clocks and wakes deadline waiters. Does nothing if the monotonic instant overflows; wall time saturates.
    pub fn advance(&self, duration: Duration) {
        self.state.send_modify(|state| {
            if let Some(monotonic) = state.monotonic.checked_add(duration) {
                state.monotonic = monotonic;
                state.utc = state.utc.saturating_add(
                    time::Duration::try_from(duration).unwrap_or(time::Duration::MAX),
                );
            }
        });
    }
}

impl Clock for TestClock {
    fn utc_now(&self) -> OffsetDateTime {
        self.state.borrow().utc
    }

    fn monotonic_now(&self) -> Instant {
        self.state.borrow().monotonic
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let mut receiver = self.state.subscribe();
        Box::pin(async move {
            loop {
                if receiver.borrow_and_update().monotonic >= deadline {
                    return;
                }
                if receiver.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}
