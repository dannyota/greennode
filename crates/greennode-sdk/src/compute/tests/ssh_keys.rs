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
        ErrorClass::Api,
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
    assert_eq!(incomplete.error().class(), ErrorClass::Api);
    assert_eq!(incomplete.error().api_error().unwrap().status(), None);
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

#[tokio::test]
async fn ambiguous_ssh_refusal_withholds_peer_text() {
    use super::super::ssh_keys::withhold_key_error;

    let server = TestServer::start(vec![http(
        "400 Bad Request",
        r#"{"data":{"privateKey":"toy-private-key"},"data":{},"message":"toy-private-key"}"#,
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
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(400));
    assert!(!api.message().contains(ISSUED));
    assert!(withhold_key_error(b"{\"message\":"));
    assert!(!withhold_key_error(br#"{"message":"Invalid public key"}"#));
}

#[tokio::test]
async fn create_requires_an_id_usable_for_cleanup() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"id":"key_1","privateKey":"toy-private-key"}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .create_ssh_key(
            &ctx,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await;
    assert!(result.is_err());
    let failure = result.unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn issued_key_hex_reflection_is_redacted_from_the_returned_name() {
    let hex = data_encoding::HEXLOWER.encode(ISSUED.as_bytes());
    let body =
        serde_json::json!({"data":{"id":"key-1","name":hex,"privateKey":ISSUED}}).to_string();
    let server = TestServer::start(vec![http("201 Created", &body)]);
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
    assert!(output.ssh_key.name == "<redacted>");
    assert!(!serde_json::to_string(&output).unwrap().contains(&hex));
}

#[tokio::test]
async fn oversized_ssh_create_refusal_uses_http_status_and_withholds_the_body() {
    let body = format!(r#"{{"message":"{ISSUED}{}"}}"#, "x".repeat(8 << 20));
    let server = TestServer::start(vec![http("409 Conflict", &body)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .create_ssh_key(
            &ctx,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(409));
    assert_eq!(api.code(), "Conflict");
    assert!(!api.message().contains(ISSUED));
    assert!(failure.error().detail::<crate::RecoveryAdvice>().is_none());
    assert!(failure.partial().is_none());
    assert_eq!(server.heads().len(), 1);
}

#[tokio::test]
async fn ssh_key_page_redacts_only_the_reflected_identifier() {
    let body = r#"{"listData":[{"id":"pre6A6B6Cpost","name":"first"},{"id":"key-2","name":"second"}],"page":1,"totalPage":1,"totalItem":2}"#;
    let server = TestServer::start(vec![response(body)]);
    let output = client_with_token(&server, "jkl")
        .list_ssh_keys(&RequestContext::new().0, ListSshKeysInput::default())
        .await
        .unwrap();
    assert_eq!(output.items.len(), 2);
    assert!(output.items[0].id == "<redacted>");
    assert_eq!(output.items[0].name, "first");
    assert_eq!(output.items[1].id, "key-2");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn import_ssh_key_keeps_the_safe_fields_of_a_refused_redirect() {
    let body = br#"{"code":"Moved","message":"temporarily moved"}"#.to_vec();
    let server = TestServer::start(vec![Response::Http {
        status: "302 Found".into(),
        headers: vec![
            ("Location".into(), "/other".into()),
            ("Content-Length".into(), body.len().to_string()),
        ],
        body,
    }]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .import_ssh_key(
            &ctx,
            ImportSshKeyInput {
                name: "example".into(),
                public_key: "ssh-rsa toy public".into(),
            },
        )
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(302), "Moved", "temporarily moved")
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn cleanup_requires_a_clean_not_found_for_the_private_delete_operation() {
    for (reply, absent) in [
        (http("404 Not Found", "{}"), true),
        (http("404 Not Found", "404 page not found"), true),
        (http("404 Not Found", r#"1 {"id":"another-key"}"#), true),
        (http("404 Not Found", "1 malformed"), false),
        (
            http("404 Not Found", r#"{"unknown":"DeleteSSHKey"}"#),
            false,
        ),
        (
            http("404 Not Found", r#"1 {"id":"RGVsZXRlU1NIS2V5"}"#),
            false,
        ),
        (
            http(
                "400 Bad Request",
                r#"{"message":"Cannot get SSH key with id key-1"}"#,
            ),
            true,
        ),
        (
            http(
                "400 Bad Request",
                r#"{"message":"Cannot get SSH key with id key-12"}"#,
            ),
            false,
        ),
        (
            http(
                "400 Bad Request",
                r#"{"message":"Cannot get SSH key with id key-1","unknown":"DeleteSSHKey"}"#,
            ),
            false,
        ),
        (TestServer::oversized("404 Not Found"), false),
    ] {
        let server = TestServer::start(vec![
            http(
                "201 Created",
                r#"{"data":{"id":"key-1","privateKey":"toy-private-key"}}"#,
            ),
            reply,
        ]);
        let mut output = client_with_token(&server, "DeleteSSHKey")
            .create_ssh_key(
                &RequestContext::new().0,
                CreateSshKeyInput {
                    name: "example".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(output.discard_after_delivery_failure().await, absent);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn mixed_json_404_screens_short_identifier_encoding_in_later_value() {
    for (body, absent) in [
        (r#"1 {"note":"prefixYWJjsuffix"}"#, true),
        (r#"1 {"id":"prefixYWJjsuffix"}"#, false),
    ] {
        let server = TestServer::start(vec![
            http(
                "201 Created",
                r#"{"data":{"id":"key-1","privateKey":"toy-private-key"}}"#,
            ),
            http("404 Not Found", body),
        ]);
        let mut output = client_with_token(&server, "abc")
            .create_ssh_key(
                &RequestContext::new().0,
                CreateSshKeyInput {
                    name: "example".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(output.discard_after_delivery_failure().await, absent);
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn ssh_cleanup_rechecks_create_ownership_with_the_delete_bearer() {
    let server = TestServer::start(vec![
        http(
            "201 Created",
            r#"{"data":{"id":"key-1","privateKey":"toy-private-key"},"unknown":"later-receipt-bearer"}"#,
        ),
        http("204 No Content", ""),
    ]);
    let mut output = receipt_client(&server, 1)
        .create_ssh_key(
            &RequestContext::new().0,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await
        .unwrap();
    assert!(!output.discard_after_delivery_failure().await);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn ssh_create_malformed_acknowledgement_keeps_statusless_decode_error() {
    let server = TestServer::start(vec![http("201 Created", "{")]);
    let failure = client(&server)
        .create_ssh_key(
            &RequestContext::new().0,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), None);
    assert!(
        failure
            .error()
            .detail::<crate::wire::product::operation::DecodeFailure>()
            .is_some()
    );
    assert_eq!(api.message(), "response failed to decode; body withheld");
}

#[tokio::test]
async fn ssh_import_refuses_reflected_acknowledgement() {
    let server = TestServer::start(vec![http(
        "201 Created",
        r#"{"data":{"id":"key-1","name":"example"},"ignored":"toy-secret"}"#,
    )]);
    let failure = client(&server)
        .import_ssh_key(
            &RequestContext::new().0,
            ImportSshKeyInput {
                name: "example".into(),
                public_key: "ssh-rsa toy public".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.write_dispatch(),
        crate::WriteDispatch::MayHaveBeenSent
    );
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn a_later_clean_bearer_does_not_restore_invalidated_ssh_ownership() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Source(AtomicUsize);
    impl crate::CredentialsProvider for Source {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>> {
            let value = if self.0.fetch_add(1, Ordering::Relaxed) == 1 {
                "invalidating-bearer"
            } else {
                "clean-bearer"
            };
            Box::pin(async move { Ok(crate::Token::new(Secret::new(value), None)) })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    let server = TestServer::start(vec![
        http(
            "201 Created",
            r#"{"data":{"id":"key-1","privateKey":"toy-private-key"},"note":"invalidating-bearer"}"#,
        ),
        http("204 No Content", ""),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_credentials_provider(Arc::new(Source(0.into())))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let mut key = Client::new(&config)
        .create_ssh_key(
            &RequestContext::new().0,
            CreateSshKeyInput {
                name: "example".into(),
            },
        )
        .await
        .unwrap();
    assert!(!key.discard_after_delivery_failure().await);
    assert!(!key.discard_after_delivery_failure().await);
    assert_eq!(server.requests(), 1);
}
