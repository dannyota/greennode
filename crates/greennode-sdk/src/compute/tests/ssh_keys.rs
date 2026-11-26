use super::*;

const ISSUED: &str = "toy-private-key";

#[tokio::test]
async fn creates_ssh_key_with_a_private_accessor_and_no_printable_secret() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"id":"key-1","name":"toy%2Dprivate%2Dkey","pubKey":"ssh-rsa toy public","privateKey":"toy-private-key"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .create_ssh_key(
            &ctx,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await
        .unwrap();
    assert!(output.private_key().expose_secret() == ISSUED);
    assert!(output.ssh_key.name == "<redacted>");
    assert!(!serde_json::to_string(&output).unwrap().contains(ISSUED));
    assert!(!format!("{output:?}").contains(ISSUED));
    assert!(server.heads()[0].starts_with(b"POST /v2/project-1/sshKeys "));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[0]).unwrap(),
        serde_json::json!({"name":"example"})
    );
}

#[tokio::test]
async fn rejects_private_key_imports_and_trims_legitimate_public_keys() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"id":"key_1","pubKey":"ssh-rsa toy public"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let invalid = client
        .import_ssh_key(
            &ctx,
            ImportSshKeyInput {
                name: "example".into(),
                public_key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.error().class(), ErrorClass::InvalidInput);
    assert_eq!(invalid.write_dispatch(), crate::WriteDispatch::NotSent);
    let output = client
        .import_ssh_key(
            &ctx,
            ImportSshKeyInput {
                name: "example".into(),
                public_key: " ssh-rsa toy public\n".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(output.ssh_key.id, "key_1");
    assert_eq!(output.ssh_key.public_key, "ssh-rsa toy public");
    assert_eq!(server.heads().len(), 1);
    assert!(server.heads()[0].starts_with(b"POST /v2/project-1/sshKeys/import "));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[0]).unwrap(),
        serde_json::json!({"name":"example","pubKey":"ssh-rsa toy public"})
    );
}

#[tokio::test]
async fn create_never_exposes_error_response_material_and_truncated_refusal_is_uncertain() {
    let server = TestServer::start(vec![
        http(
            "409 Conflict",
            r#"{"message":"toy-private-key","privateKey":"toy-private-key"}"#,
        ),
        Response::RawClose(b"HTTP/1.1 409 Conflict\r\nContent-Length: 100\r\n\r\n{".to_vec()),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let input = CreateSshKeyInput {
        name: "example".into(),
    };
    let refused = client
        .create_ssh_key(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(refused.error().api_error().unwrap().status(), Some(409));
    assert!(
        !refused
            .error()
            .api_error()
            .unwrap()
            .message()
            .contains(ISSUED)
    );
    assert!(refused.partial().is_none());
    let uncertain = client.create_ssh_key(&ctx, input).await.unwrap_err();
    assert_eq!(
        uncertain.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert_eq!(
        uncertain.error().class(),
        ErrorClass::NotSettled,
        "observed HTTP status {:?}",
        super::super::mutation::status(uncertain.error())
    );
    assert!(
        uncertain
            .error()
            .detail::<crate::RecoveryAdvice>()
            .unwrap()
            .message()
            .contains("list-ssh-keys")
    );
    assert!(uncertain.partial().is_none());
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn delete_recognizes_only_the_requested_key_in_the_provider_refusal() {
    let server = TestServer::start(vec![
        http(
            "400 Bad Request",
            r#"{"message":"Cannot get ssh key with id key-10"}"#,
        ),
        http(
            "400 Bad Request",
            r#"{"message":"Cannot get ssh key with id key-1"}"#,
        ),
        Response::RawClose(b"HTTP/1.1 404 Not Found\r\nContent-Length: 100\r\n\r\n{".to_vec()),
        http("204 No Content", ""),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server);
    let input = DeleteSshKeyInput {
        ssh_key_id: "key-1".into(),
    };
    let other = client
        .delete_ssh_key(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(!other.error().has_class(ErrorClass::NotFound));
    let absent = client
        .delete_ssh_key(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(absent.error().has_identity(ErrorClass::NotFound));
    assert_eq!(absent.error().api_error().unwrap().status(), Some(400));
    let incomplete = client
        .delete_ssh_key(&ctx, input.clone())
        .await
        .unwrap_err();
    assert_eq!(incomplete.error().class(), ErrorClass::NotSettled);
    assert_eq!(incomplete.error().api_error().unwrap().status(), Some(404));
    assert!(incomplete.partial().is_none());
    client.delete_ssh_key(&ctx, input).await.unwrap();
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"DELETE /v2/project-1/sshKeys/key-1 "))
    );
    assert!(server.bodies().iter().all(Vec::is_empty));
}

#[tokio::test]
async fn import_rejects_unexpected_private_key_alias_without_public_partial() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"id":"key-1","pubKey":"toy-private-key","PrivateKey":"toy-private-key"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .import_ssh_key(
            &ctx,
            ImportSshKeyInput {
                name: "example".into(),
                public_key: "ssh-rsa toy".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert!(!format!("{failure:?}").contains(ISSUED));
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
}

#[tokio::test]
async fn rejects_embedded_short_credential_in_issued_key_id() {
    let server = TestServer::start(vec![
        http(
            "201 Created",
            r#"{"data":{"id":"key-Y2F0-id","privateKey":"toy-private-key"}}"#,
        ),
        http(
            "201 Created",
            r#"{"data":{"id":"key-1","privateKey":"toy-private-key"}}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client_with_token(&server, "cat");
    let input = CreateSshKeyInput {
        name: "example".into(),
    };
    let refused = client
        .create_ssh_key(&ctx, input.clone())
        .await
        .unwrap_err();
    assert!(refused.partial().is_none());
    let valid = client.create_ssh_key(&ctx, input).await.unwrap();
    assert_eq!(valid.ssh_key.id, "key-1");
    assert_eq!(server.heads().len(), 2);
}
