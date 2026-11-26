use super::{client, response};
use crate::iam::{CreateGroupInput, DeleteGroupInput, UpdateGroupInput};
use crate::wire::test_server::Server;
use crate::{ErrorClass, RequestContext, WriteDispatch};
use serde_json::{Value, json};

#[tokio::test]
async fn create_and_update_confirm_on_the_iam_endpoint() {
    let existing = br#"{"id":"group-1","name":"team","description":"old"}"#;
    let updated = br#"{"id":"group-1","name":"team","description":"new"}"#;
    let server = Server::start(vec![
        response("201 Created", br#"{"id":"group-1"}"#),
        response("200 OK", existing),
        response("200 OK", existing),
        response("502 Bad Gateway", b"{}"),
        response("204 No Content", b""),
        response("200 OK", updated),
    ]);
    let client = client(&server, "toy-bearer");
    let (ctx, _) = RequestContext::new();
    let created = client
        .create_group(&ctx, CreateGroupInput::new("team"))
        .await
        .unwrap();
    assert_eq!(created.group.id, "group-1");
    let mut input = UpdateGroupInput::new("group-1");
    input.description = Some("new".into());
    let changed = client.update_group(&ctx, input).await.unwrap();
    assert_eq!(changed.group.description, "new");
    let heads = server.heads();
    for (head, route) in heads.iter().zip([
        "POST /iam/policies-api/v1/groups ",
        "GET /iam/policies-api/v1/groups/group-1 ",
        "GET /iam/policies-api/v1/groups/group-1 ",
        "PATCH /iam/policies-api/v1/groups/group-1 ",
        "PATCH /iam/policies-api/v1/groups/group-1 ",
        "GET /iam/policies-api/v1/groups/group-1 ",
    ]) {
        assert!(head.starts_with(route.as_bytes()));
    }
    assert_eq!(heads.len(), 6);
    let bodies = server.bodies();
    assert!(
        serde_json::from_slice::<Value>(&bodies[0]).ok()
            == Some(json!({
                "name":"team", "description":"", "mode":"iam"
            }))
    );
    assert!(
        serde_json::from_slice::<Value>(&bodies[3]).ok()
            == Some(json!({
                "name":"team", "description":"new"
            }))
    );
    assert!(bodies[3] == bodies[4]);
}

#[tokio::test]
async fn create_never_replays_or_confirms_an_ambiguous_identity() {
    let (ctx, _) = RequestContext::new();
    for (status, body) in [
        ("502 Bad Gateway", b"{}".as_slice()),
        ("201 Created", br#"{"id":"group-1","ID":"group-1"}"#),
        ("201 Created", br#"{"id":"toy-bearer"}"#),
        ("201 Created", br#"{"id":"group_1"}"#),
    ] {
        let server = Server::start(vec![response(status, body)]);
        let err = client(&server, "toy-bearer")
            .create_group(&ctx, CreateGroupInput::new("team"))
            .await
            .unwrap_err();
        assert_eq!(err.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        let (error, partial, _) = err.into_parts();
        assert!(partial.is_none());
        assert!(!error.to_string().contains("toy-bearer"));
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn settled_partials_screen_retained_fields_and_bad_input_sends_nothing() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"id":"group-1","name":"team","description":"toy%2Fbearer"}"#,
        ),
        response("204 No Content", b""),
        response("404 Not Found", b"{}"),
    ]);
    let client = client(&server, "toy/bearer");
    let failed = client
        .update_group(&ctx, UpdateGroupInput::new("group-1"))
        .await
        .unwrap_err();
    let (error, partial, dispatch) = failed.into_parts();
    assert!(error.has_identity(ErrorClass::NotSettled));
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    let output = serde_json::to_value(partial.unwrap()).unwrap();
    assert_eq!(output["Group"]["ID"], "group-1");
    assert_eq!(output["Group"]["Description"], "<redacted>");
    let mut rejected = UpdateGroupInput::new("group-1");
    rejected.description = Some("toy%2Fbearer".into());
    let err = client.update_group(&ctx, rejected).await.unwrap_err();
    assert_eq!(err.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn delete_requires_proven_empty_iam_membership() {
    let (ctx, _) = RequestContext::new();
    let caller = br#"{"userId":"user-1","userType":"iam-user"}"#;
    for body in [
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":["policy-1"]}"#.as_slice(),
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":null}"#,
        br#"{"id":"group-1","mode":"idp","iamUsers":[],"policies":[]}"#,
        br#"{"id":"group-2","mode":"iam","iamUsers":[],"policies":[]}"#,
        br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":[],"unknown":true}"#,
    ] {
        let server = Server::start(vec![response("200 OK", caller), response("200 OK", body)]);
        let failed = client(&server, "toy-bearer")
            .delete_group(&ctx, DeleteGroupInput::new("group-1"))
            .await
            .unwrap_err();
        assert_eq!(failed.write_dispatch(), WriteDispatch::NotSent);
        let heads = server.heads();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].starts_with(b"GET /dashboard/accounts-api/v1/auth/userinfo "));
        assert!(heads[1].starts_with(b"GET /iam/policies-api/v1/groups/group-1 "));
    }
    let server = Server::start(vec![
        response("200 OK", caller),
        response(
            "200 OK",
            br#"{"id":"group-1","mode":"iam","iamUsers":[],"policies":[]}"#,
        ),
        response("204 No Content", b""),
    ]);
    client(&server, "toy-bearer")
        .delete_group(&ctx, DeleteGroupInput::new("group-1"))
        .await
        .unwrap();
    assert!(server.heads()[2].starts_with(b"DELETE /iam/policies-api/v1/groups/group-1 "));
}
