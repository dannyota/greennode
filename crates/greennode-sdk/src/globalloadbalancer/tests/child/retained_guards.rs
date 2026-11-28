use super::super::tests::{TOKEN, child::*, response};
use super::{Child, Write};
use crate::wire::product::evidence::Evidence;
use crate::{ConfigOptions, RequestContext, Secret, WriteDispatch};
use serde_json::{Value, json};
use std::{
    future::{Future, ready},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[tokio::test]
async fn retained_pool_status_is_rechecked_with_the_delete_bearer() {
    use crate::globalloadbalancer::write::{guard_operation, write_operation};
    use crate::wire::product::guard::Guarded;
    let replies = vec![
        ok(&json!({"id":"pool-1", "status":"ACTIVE"})),
        response("400 Bad Request", "{}"),
    ];
    let options = rotating_at(2, "ACTIVE");
    let (client, glb, _) = servers_with_auth(replies, vec![], true, options);
    let id = "globalloadbalancer.DeletePool";
    let mut write = Write {
        client: &client,
        id,
        evidence: Evidence::new(),
    };
    let path = "v1/global-load-balancers/{}/global-pools/{}";
    let ids = &["glb-1", "pool-1"];
    let ctx = RequestContext::new().0;
    let reply = write
        .guard_on(
            &ctx,
            &guard_operation(id, path),
            client.session.endpoints().global_load_balancer.as_ref(),
            ids,
        )
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(reply.clean_receipt().unwrap().body()).unwrap();
    let pool = Guarded::latest(body, &write.evidence).unwrap();
    assert_eq!(pool.recheck(&write.evidence).unwrap()["status"], "ACTIVE");
    let failure = write
        .request(
            &ctx,
            &write_operation(id, path, http::Method::DELETE),
            ids,
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(glb.requests(), 1);
}

struct RotateAt {
    acquisitions: AtomicUsize,
    at: usize,
    bearer: &'static str,
}

impl crate::CredentialsProvider for RotateAt {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
        let acquisition = self.acquisitions.fetch_add(1, Ordering::Relaxed) + 1;
        let bearer = if acquisition < self.at {
            TOKEN
        } else {
            self.bearer
        };
        Box::pin(ready(Ok(crate::Token::new(Secret::new(bearer), None))))
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
        Box::pin(ready(Ok(())))
    }
}

pub(super) fn rotating_at(at: usize, bearer: &'static str) -> ConfigOptions {
    ConfigOptions::default().with_credentials_provider(Arc::new(RotateAt {
        acquisitions: 0.into(),
        at,
        bearer,
    }))
}

pub(super) fn child(client: &super::Client) -> Child<'_> {
    Child(Write {
        client,
        id: "globalloadbalancer.CreatePool",
        evidence: Evidence::new(),
    })
}

#[tokio::test]
async fn glb_guard_refusal_does_not_invent_an_ok_status() {
    let mut fixture = child_fixture();
    fixture["parent"]["note"] = json!(TOKEN);
    let options = ConfigOptions::default().with_static_token(Secret::new(TOKEN));
    let (client, glb, _) = servers_with_auth(vec![ok(&fixture["parent"])], vec![], true, options);
    let failure = client
        .create_pool(&RequestContext::new().0, pool_input())
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(
        failure
            .error()
            .api_error()
            .and_then(crate::ApiError::status),
        None
    );
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::RefusedGuard>()
            .is_some()
    );
    assert_eq!(glb.requests(), 1);
}

#[tokio::test]
async fn glb_pool_create_rechecks_acknowledgement_through_final_confirmation() {
    for (at, bearer, reflects, requests) in [
        (10, "pool-1", true, 6),
        (15, "toy-ack-only", true, 11),
        (15, "toy-clean-bearer", false, 12),
    ] {
        let mut fixture = child_fixture();
        fixture["pool_accept"]["note"] = json!("toy-ack-only");
        let mut replies = catalog(&fixture);
        replies.extend([
            empty(),
            empty(),
            accepted(&fixture["pool_accept"]),
            ok(&fixture["parent"]),
        ]);
        replies.extend(chain(&fixture));
        replies.push(empty());
        let (client, glb, regional) =
            servers_with_auth(replies, placement(&fixture), true, rotating_at(at, bearer));
        let result = client
            .create_pool(&RequestContext::new().0, pool_input())
            .await;
        if reflects {
            let Err(failure) = result else {
                panic!("reflected acknowledgement was accepted");
            };
            assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
            assert!(failure.partial().is_none());
        } else {
            assert_eq!(result.unwrap().pool.id, "pool-1");
        }
        assert_eq!(glb.requests(), requests);
        assert_eq!(regional.requests(), 3);
    }
}

#[tokio::test]
async fn glb_final_catalog_read_rechecks_the_region() {
    let mut fixture = child_fixture();
    fixture["regions"][0]["note"] = json!("toy-final-bearer");
    let (client, glb, _) = servers_with_auth(
        catalog(&fixture),
        vec![],
        true,
        rotating_at(3, "toy-final-bearer"),
    );
    let mut run = child(&client);
    let ctx = RequestContext::new().0;
    let parent = run.parent(&ctx, "glb-1").await.unwrap();
    assert!(run.region(&ctx, &parent).await.is_err());
    assert_eq!(glb.requests(), 3);
}

#[tokio::test]
async fn glb_final_placement_read_rechecks_the_server() {
    let mut fixture = child_fixture();
    fixture["server"]["note"] = json!("toy-final-bearer");
    let (client, _, regional) = servers_with_auth(
        vec![],
        placement(&fixture),
        true,
        rotating_at(3, "toy-final-bearer"),
    );
    let mut run = child(&client);
    let ctx = RequestContext::new().0;
    assert!(
        run.placement(&ctx, "project-1", "server-1", "vpc-1", "sub-1")
            .await
            .is_err()
    );
    assert_eq!(regional.requests(), 3);
}

#[tokio::test]
async fn glb_final_chain_read_rechecks_the_pool_listing() {
    let mut fixture = child_fixture();
    fixture["pool"]["note"] = json!("toy-final-bearer");
    let mut replies = chain(&fixture);
    replies[1] = ok(&child_fixture()["pool"]);
    let (client, glb, _) =
        servers_with_auth(replies, vec![], true, rotating_at(4, "toy-final-bearer"));
    assert!(
        child(&client)
            .chain(&RequestContext::new().0, "glb-1", "pool-1", None, "")
            .await
            .is_err()
    );
    assert_eq!(glb.requests(), 4);
}

#[tokio::test]
async fn glb_confirmation_can_poll_for_the_full_fifteen_minutes() {
    use crate::{Clock, TestClock};
    use std::time::Duration;
    use tokio::time::Instant;
    #[derive(Clone)]
    struct PollClock {
        clock: TestClock,
        deadline: Instant,
    }
    impl Clock for PollClock {
        fn utc_now(&self) -> time::OffsetDateTime {
            self.clock.utc_now()
        }
        fn monotonic_now(&self) -> Instant {
            self.clock.monotonic_now()
        }
        fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            let delay = deadline.saturating_duration_since(self.monotonic_now());
            if !delay.is_zero() && delay <= Duration::from_secs(5) && deadline < self.deadline {
                Box::pin(async move {
                    self.clock.advance(delay);
                })
            } else {
                self.clock.sleep_until(deadline)
            }
        }
    }
    let clock = TestClock::new(time::OffsetDateTime::UNIX_EPOCH);
    let start = clock.monotonic_now();
    let window = Duration::from_secs(900);
    let mut fixture = child_fixture();
    let mut replies = catalog(&fixture);
    replies.extend([empty(), empty(), accepted(&fixture["pool_accept"])]);
    fixture["pool"]["status"] = json!("PENDING");
    let (started, final_read) = tokio::sync::oneshot::channel();
    let polls = 180;
    for _ in 0..polls - 1 {
        replies.push(ok(&fixture["parent"]));
        replies.extend(chain(&fixture));
        replies.push(empty());
    }
    replies.push(ok(&fixture["parent"]));
    replies.extend(chain(&fixture));
    replies.push(crate::wire::test_server::Response::StallBeforeHead(started));
    let options = ConfigOptions::default()
        .with_static_token(Secret::new(TOKEN))
        .with_clock(Arc::new(PollClock {
            clock: clock.clone(),
            deadline: start + window,
        }));
    let (client, glb, regional) = servers_with_auth(replies, placement(&fixture), true, options);
    let ctx = RequestContext::new().0;
    let confirm = client.create_pool(&ctx, pool_input());
    tokio::pin!(confirm);
    let Ok(result) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            result = &mut confirm => result,
            started = final_read => {
                started.unwrap();
                let remaining = (start + window).duration_since(clock.monotonic_now());
                clock.advance(remaining);
                confirm.await
            }
        }
    })
    .await
    else {
        panic!(
            "poll stalled after {} GLB requests, {} regional requests, and {:?} of virtual time",
            glb.requests(),
            regional.requests(),
            clock.monotonic_now().duration_since(start)
        );
    };
    let failure = result.unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        glb.requests(),
        6 + polls * 6,
        "regional requests: {}; virtual time: {:?}; class: {:?}",
        regional.requests(),
        clock.monotonic_now().duration_since(start),
        failure.error().class()
    );
    assert_eq!(clock.monotonic_now().duration_since(start), window);
    assert_eq!(failure.partial().unwrap().pool.id, "pool-1");
}
