use super::models::*;
use super::tests::{client, reply};
use crate::testing::{Response, Server};
use crate::{ErrorClass, RequestContext};

async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(60), future)
        .await
        .expect("the 30-second deadline did not end the read")
}

#[tokio::test]
async fn single_protected_read_has_its_own_deadline() {
    let _reset = DeadlineReset::read(std::time::Duration::from_millis(200));
    let (started, _hold) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(started)]);
    let (ctx, _) = RequestContext::new();
    let error =
        within(client(&server).get_security_group(&ctx, GetSecurityGroupInput::new("sg-1")))
            .await
            .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::DeadlineExceeded);
}

#[tokio::test]
async fn rule_walk_has_one_deadline_across_its_reads() {
    let _reset = DeadlineReset::walk(std::time::Duration::from_millis(200));
    let (started, _hold) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"listData":[{"id":"sg-1"}],"page":1,"pageSize":1,"totalPage":1,"totalItem":1}"#,
        ),
        Response::StallBeforeHead(started),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = within(
        client(&server)
            .list_all_security_group_rules(&ctx, ListAllSecurityGroupRulesInput::default()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::DeadlineExceeded);
}

struct DeadlineReset;

impl DeadlineReset {
    fn read(deadline: std::time::Duration) -> Self {
        super::wire::READ_DEADLINE.with(|cell| cell.set(Some(deadline)));
        Self
    }

    fn walk(deadline: std::time::Duration) -> Self {
        super::wire::WALK_DEADLINE.with(|cell| cell.set(Some(deadline)));
        Self
    }
}

impl Drop for DeadlineReset {
    fn drop(&mut self) {
        super::wire::READ_DEADLINE.with(|cell| cell.set(None));
        super::wire::WALK_DEADLINE.with(|cell| cell.set(None));
    }
}
