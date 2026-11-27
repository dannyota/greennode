use super::*;

#[tokio::test]
async fn retained_pool_status_is_rechecked_with_the_delete_bearer() {
    use std::{
        future::{Future, ready},
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Rotating(AtomicUsize);
    impl crate::CredentialsProvider for Rotating {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            let bearer = if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
                TOKEN
            } else {
                "ACTIVE"
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
    use crate::globalloadbalancer::write::{Write, guard_operation, write_operation};
    use crate::wire::product::{evidence::Evidence, guard::Guarded};
    let replies = vec![
        ok(&json!({"id":"pool-1", "status":"ACTIVE"})),
        response("400 Bad Request", "{}"),
    ];
    let options =
        ConfigOptions::default().with_credentials_provider(Arc::new(Rotating(AtomicUsize::new(0))));
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
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
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
