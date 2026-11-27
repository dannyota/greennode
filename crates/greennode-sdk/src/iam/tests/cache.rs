use super::{client, response};
use crate::iam::policy_guard::Guard;
use crate::iam::writes::write_operation;
use crate::iam::{Api, GetCallerIdentityOutput};
use crate::wire::product::evidence::Evidence;
use crate::wire::product::redirect::Retry;
use crate::wire::test_server::Server;
use crate::{RequestContext, Secret};
use http::{Method, StatusCode};

const CALLER: &[u8] =
    br#"{"userId":"usr-1","userType":"iam-user","username":"alice","accountId":7}"#;
const CATALOG: &[u8] = br#"[{"action":"CreatePolicy","label":"Write","resources":[]}]"#;

#[tokio::test]
async fn caller_is_read_once_per_client_and_shared_by_clones() {
    let server = Server::start(vec![response("200 OK", CALLER)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    for c in [&client, &client.clone(), &client] {
        let caller = c
            .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
            .await
            .unwrap();
        assert_eq!(caller.user_id, "usr-1");
    }
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn cached_caller_reflecting_a_new_credential_is_refused() {
    let server = Server::start(vec![response("200 OK", CALLER)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("usr-1"));
    let refused = client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut evidence)
        .await;
    let text = refused
        .unwrap_err()
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message()
        .to_owned();
    assert!(
        text.contains("iam: iam.GetCallerIdentity: unsafe cached guard response"),
        "{text}"
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn write_actions_are_read_once_and_rechecked() {
    let server = Server::start(vec![response("200 OK", CATALOG)]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let first = client
        .guard_write_actions::<()>(&ctx, &mut Evidence::new())
        .await
        .unwrap();
    let second = client
        .guard_write_actions::<()>(&ctx, &mut Evidence::new())
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(server.requests(), 1);
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("iam:createpolicy"));
    let refused = client
        .guard_write_actions::<()>(&ctx, &mut evidence)
        .await
        .unwrap_err();
    let text = refused
        .error()
        .detail::<crate::RecoveryAdvice>()
        .unwrap()
        .message()
        .to_owned();
    assert!(
        text.contains("iam: iam.ListActions: unsafe cached guard response"),
        "{text}"
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn policy_guard_shares_caller_and_actions_across_operations() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", CATALOG),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    for _ in 0..2 {
        let mut evidence = Evidence::new();
        let mut guard = Guard::new::<()>(&client, &ctx, "iam.CreatePolicy", &mut evidence)
            .await
            .unwrap();
        guard.load_actions::<()>().await.unwrap();
    }
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn concurrent_first_uses_send_one_caller_and_one_actions_read() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", CATALOG),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let (mut e1, mut e2, mut e3, mut e4) = (
        Evidence::new(),
        Evidence::new(),
        Evidence::new(),
        Evidence::new(),
    );
    let (a, b) = tokio::join!(
        client.require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut e1),
        client.require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut e2),
    );
    let (c, d) = tokio::join!(
        client.guard_write_actions::<()>(&ctx, &mut e3),
        client.guard_write_actions::<()>(&ctx, &mut e4),
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok() && d.is_ok());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn cached_values_are_rechecked_before_a_write_is_sent() {
    let server = Server::start(vec![
        response("200 OK", CALLER),
        response("200 OK", CATALOG),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    client
        .require_classified_caller::<()>(&ctx, "iam.DeleteGroup", &mut Evidence::new())
        .await
        .unwrap();
    client
        .guard_write_actions::<()>(&ctx, &mut Evidence::new())
        .await
        .unwrap();
    let operation = write_operation(
        "iam.DeleteGroup",
        Method::POST,
        "policies-api/v1/groups/x",
        Retry::Once,
        &[StatusCode::OK],
    );
    for secret in ["usr-1", "iam:createpolicy"] {
        let mut evidence = Evidence::new();
        evidence.add_secret(&Secret::new(secret));
        let refused = client
            .write::<GetCallerIdentityOutput, ()>(
                &ctx,
                Api::Policies,
                &operation,
                &[],
                None,
                &mut evidence,
            )
            .await
            .unwrap_err();
        assert_eq!(refused.write_dispatch(), crate::WriteDispatch::NotSent);
        assert_eq!(server.requests(), 2);
    }
}
