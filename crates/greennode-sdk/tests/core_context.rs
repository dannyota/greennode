use std::{sync::Arc, time::Duration};

use greennode_sdk::{Clock, ContextEnd, RequestContext, TestClock};
use time::OffsetDateTime;

#[tokio::test]
async fn cancellation_reaches_clones_and_children() {
    let clock = Arc::new(TestClock::new(OffsetDateTime::UNIX_EPOCH));
    let (ctx, cancel) = RequestContext::new(clock.clone());
    let child = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(5));
    let clone = ctx.clone();
    assert_eq!(ctx.check(), None);
    let waiter = tokio::spawn(async move { child.wait().await });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    cancel.cancel();
    assert_eq!(clone.check(), Some(ContextEnd::Cancelled));
    assert_eq!(waiter.await.unwrap(), ContextEnd::Cancelled);
}

#[tokio::test]
async fn child_deadline_cannot_extend_parent() {
    let clock = Arc::new(TestClock::new(OffsetDateTime::UNIX_EPOCH));
    let (ctx, _) = RequestContext::new(clock.clone());
    let now = clock.monotonic_now();
    let parent = ctx.child_with_deadline(now + Duration::from_secs(3));
    let child = parent.child_with_deadline(now + Duration::from_secs(8));
    assert_eq!(child.deadline(), parent.deadline());
    let earlier = parent.child_with_deadline(now + Duration::from_secs(1));
    assert_eq!(earlier.deadline(), Some(now + Duration::from_secs(1)));
}

#[tokio::test]
async fn expiration_uses_context_clock() {
    let clock = Arc::new(TestClock::new(OffsetDateTime::UNIX_EPOCH));
    let (ctx, _) = RequestContext::new(clock.clone());
    let ctx = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(2));
    assert_eq!(ctx.check(), None);
    let waiting = ctx.clone();
    let waiter = tokio::spawn(async move { waiting.wait().await });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    clock.advance(Duration::from_secs(2));
    assert_eq!(ctx.check(), Some(ContextEnd::DeadlineExceeded));
    assert_eq!(waiter.await.unwrap(), ContextEnd::DeadlineExceeded);
    assert_eq!(
        clock.utc_now(),
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(2)
    );
}

#[tokio::test]
async fn sleep_completes_only_after_clock_advances_to_deadline() {
    let clock = Arc::new(TestClock::new(OffsetDateTime::UNIX_EPOCH));
    let deadline = clock.monotonic_now() + Duration::from_secs(2);
    let sleeper = clock.clone();
    let task = tokio::spawn(async move { sleeper.sleep_until(deadline).await });
    tokio::task::yield_now().await;
    clock.advance(Duration::from_secs(1));
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    clock.advance(Duration::from_secs(1));
    task.await.unwrap();
}
