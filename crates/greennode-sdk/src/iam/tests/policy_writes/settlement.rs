use super::*;

#[tokio::test]
async fn policy_replacement_reports_a_definite_refusal_when_status_is_hidden() {
    let policy = br#"{"id":"pol-1","name":"reader","description":"old","manager":"user","statements":[{"effect":"allow","actions":["iam:ListActions"],"resources":["*"]}]}"#;
    let server = Server::start(vec![
        caller(),
        response("200 OK", policy),
        catalog(),
        response("200 OK", b"[]"),
        response("200 OK", b"[]"),
        response("200 OK", b"[]"),
        response("403 Forbidden", b"{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = UpdatePolicyInput::new(POLICY_ID);
    input.description = Some("new".into());
    let failure = client(&server, "403")
        .update_policy(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 7);
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(failure.error().has_class(ErrorClass::PermissionDenied));
    assert!(!failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(failure.error().api_error().unwrap().status(), None);
}
