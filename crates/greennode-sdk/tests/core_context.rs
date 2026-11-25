use std::{
    future::Future,
    task::{Context, Waker},
    time::Duration,
};

use greennode_sdk::{Clock, ContextEnd, RequestContext, TestClock};
use time::OffsetDateTime;

#[tokio::test]
async fn cancellation_reaches_clones_and_children() {
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (ctx, cancel) = RequestContext::new();
    let child = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(5));
    let clone = ctx.clone();
    assert_eq!(ctx.check(&clock), None);
    let waiter = child.wait(&clock);
    tokio::pin!(waiter);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    cancel.cancel();
    assert_eq!(clone.check(&clock), Some(ContextEnd::Cancelled));
    assert_eq!(
        waiter.as_mut().poll(&mut cx),
        std::task::Poll::Ready(ContextEnd::Cancelled)
    );
}

#[tokio::test]
async fn child_deadline_cannot_extend_parent() {
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (ctx, _) = RequestContext::new();
    let now = clock.monotonic_now();
    let parent = ctx.child_with_deadline(now + Duration::from_secs(3));
    let child = parent.child_with_deadline(now + Duration::from_secs(8));
    assert_eq!(child.deadline(), parent.deadline());
    let earlier = parent.child_with_deadline(now + Duration::from_secs(1));
    assert_eq!(earlier.deadline(), Some(now + Duration::from_secs(1)));
}

#[tokio::test]
async fn expiration_uses_supplied_clock() {
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let other = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let (ctx, _) = RequestContext::new();
    let ctx = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(2));
    assert_eq!(ctx.check(&clock), None);
    let waiter = ctx.wait(&clock);
    tokio::pin!(waiter);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    other.advance(Duration::from_secs(4));
    assert_eq!(ctx.check(&clock), None);
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    clock.advance(Duration::from_secs(2));
    assert_eq!(ctx.check(&clock), Some(ContextEnd::DeadlineExceeded));
    assert_eq!(waiter.await, ContextEnd::DeadlineExceeded);
    assert_eq!(
        clock.utc_now(),
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(2)
    );
}

#[tokio::test]
async fn sleep_completes_only_after_clock_advances_to_deadline() {
    let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH);
    let deadline = clock.monotonic_now() + Duration::from_secs(2);
    let sleeper = clock.sleep_until(deadline);
    tokio::pin!(sleeper);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(sleeper.as_mut().poll(&mut cx).is_pending());
    clock.advance(Duration::from_secs(1));
    assert!(sleeper.as_mut().poll(&mut cx).is_pending());
    clock.advance(Duration::from_secs(1));
    sleeper.await;
}
