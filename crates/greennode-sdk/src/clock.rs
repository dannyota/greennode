use std::{future::Future, pin::Pin, time::Duration};

use time::OffsetDateTime;
use tokio::{sync::watch, time::Instant};

pub trait Clock: Send + Sync {
    fn utc_now(&self) -> OffsetDateTime;
    fn monotonic_now(&self) -> Instant;
    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

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

#[derive(Clone)]
pub struct TestClock {
    state: watch::Sender<TimeState>,
}

impl TestClock {
    pub fn new(utc: OffsetDateTime) -> Self {
        let (state, _) = watch::channel(TimeState {
            utc,
            monotonic: Instant::now(),
        });
        Self { state }
    }

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
