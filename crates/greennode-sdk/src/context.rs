use std::{
    future::{Future, poll_fn},
    sync::Arc,
    task::Poll,
};

use tokio::{sync::watch, time::Instant};

use crate::Clock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextEnd {
    Cancelled,
    DeadlineExceeded,
}

#[derive(Clone)]
pub struct RequestContext {
    clock: Arc<dyn Clock>,
    cancelled: watch::Receiver<bool>,
    sender: watch::Sender<bool>,
    deadline: Option<Instant>,
}

#[derive(Clone)]
pub struct CancelHandle {
    cancelled: watch::Sender<bool>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
}

impl RequestContext {
    pub fn new(clock: Arc<dyn Clock>) -> (Self, CancelHandle) {
        let (cancelled, receiver) = watch::channel(false);
        (
            Self {
                clock,
                cancelled: receiver,
                sender: cancelled.clone(),
                deadline: None,
            },
            CancelHandle { cancelled },
        )
    }

    pub fn child_with_deadline(&self, deadline: Instant) -> Self {
        Self {
            clock: self.clock.clone(),
            cancelled: self.cancelled.clone(),
            sender: self.sender.clone(),
            deadline: Some(
                self.deadline
                    .map_or(deadline, |parent| parent.min(deadline)),
            ),
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn check(&self) -> Option<ContextEnd> {
        if *self.cancelled.borrow() {
            Some(ContextEnd::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| self.clock.monotonic_now() >= deadline)
        {
            Some(ContextEnd::DeadlineExceeded)
        } else {
            None
        }
    }

    pub async fn wait(&self) -> ContextEnd {
        let mut cancelled = self.cancelled.clone();
        let mut sleep = self
            .deadline
            .map(|deadline| self.clock.sleep_until(deadline));
        loop {
            if let Some(end) = self.check() {
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
