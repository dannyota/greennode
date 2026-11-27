use std::{
    future::{Future, poll_fn},
    task::Poll,
};

use tokio::{sync::watch, time::Instant};

use crate::Clock;

/// Reason an operation should stop.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ContextEnd {
    /// Cancellation was requested.
    Cancelled,
    /// The monotonic deadline was reached.
    DeadlineExceeded,
}

crate::error::debug_record!(ContextEnd; Cancelled, DeadlineExceeded);

/// Shared cancellation state and an optional monotonic deadline.
#[derive(Clone)]
pub struct RequestContext {
    cancelled: watch::Receiver<bool>,
    sender: watch::Sender<bool>,
    deadline: Option<Instant>,
}

/// A handle that cancels the associated context and its children.
#[derive(Clone)]
pub struct CancelHandle {
    cancelled: watch::Sender<bool>,
}

impl CancelHandle {
    /// Signals cancellation to all contexts sharing this handle.
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
}

impl RequestContext {
    /// Keeps response screening active until the supplied operation and its diagnostic finish.
    #[doc(hidden)]
    pub async fn with_response_screening<T>(future: impl std::future::Future<Output = T>) -> T {
        crate::wire::product::exit::scope(future).await
    }

    /// Checks the exact bounded bytes an operation is about to publish.
    #[doc(hidden)]
    pub fn response_bytes_reflect(bytes: &[u8]) -> bool {
        crate::wire::product::exit::bytes_reflect(bytes)
    }

    /// Withholds a diagnostic that reflects credentials observed in this operation.
    #[doc(hidden)]
    pub fn screened_response_text(text: &str) -> &str {
        crate::wire::product::exit::screen_text(text)
    }

    /// Creates an uncancelled context without a deadline and its cancellation handle.
    pub fn new() -> (Self, CancelHandle) {
        let (cancelled, receiver) = watch::channel(false);
        (
            Self {
                cancelled: receiver,
                sender: cancelled.clone(),
                deadline: None,
            },
            CancelHandle { cancelled },
        )
    }

    /// Shares cancellation and uses the earlier of the parent and supplied deadlines.
    pub fn child_with_deadline(&self, deadline: Instant) -> Self {
        Self {
            cancelled: self.cancelled.clone(),
            sender: self.sender.clone(),
            deadline: Some(
                self.deadline
                    .map_or(deadline, |parent| parent.min(deadline)),
            ),
        }
    }

    /// Returns the effective monotonic deadline, if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Checks cancellation before the deadline; returns `None` while active.
    pub fn check(&self, clock: &dyn Clock) -> Option<ContextEnd> {
        if *self.cancelled.borrow() {
            Some(ContextEnd::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| clock.monotonic_now() >= deadline)
        {
            Some(ContextEnd::DeadlineExceeded)
        } else {
            None
        }
    }

    /// Waits until cancelled or the deadline is reached. Cancellation takes precedence when both apply.
    pub async fn wait(&self, clock: &dyn Clock) -> ContextEnd {
        let mut cancelled = self.cancelled.clone();
        let mut sleep = self.deadline.map(|deadline| clock.sleep_until(deadline));
        loop {
            if let Some(end) = self.check(clock) {
                return end;
            }
            let changed = cancelled.changed();
            tokio::pin!(changed);
            poll_fn(|cx| {
                if changed.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
                if let Some(sleep) = sleep.as_mut()
                    && sleep.as_mut().poll(cx).is_ready()
                {
                    return Poll::Ready(());
                }
                Poll::Pending
            })
            .await;
        }
    }
}
