use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server) -> Client {
    client_with(
        server,
        ConfigOptions::default().with_static_token(Secret::new("synthetic-registry-token")),
    )
}

fn client_with(server: &Server, options: ConfigOptions) -> Client {
    let config = Config::new(options.with_region("hcm-3".into()).with_endpoint_overrides(
        EndpointOverrides {
            container_registry: Some(server.origin.clone()),
            ..Default::default()
        },
    ))
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn delete_repository_requires_explicit_zero_image_count() {
    let server = Server::start(vec![response("200 OK", r#"{"uuid":"repo-1"}"#)]);
    let error = client(&server)
        .delete_repository(
            &RequestContext::new().0,
            DeleteRepositoryInput::new("repo-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn create_user_preserves_only_the_secret_after_reflected_candidate() {
    let server = Server::start(vec![
        response("200 OK", r#"[{"uuid":"policy-1","action":"pull"}]"#),
        response("200 OK", r#"{"data":[],"totalPage":0,"totalItem":0}"#),
        response("200 OK", r#"{"secretKey":"issued-secret-canary"}"#),
        response(
            "200 OK",
            r#"{"data":[{"uuid":"robot-1","name":"app-ci","description":"issued-secret-canary"}],"totalPage":1,"totalItem":1}"#,
        ),
    ]);
    let error = client(&server)
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["pull".into()])],
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        error.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::UserNotFound)
    );
    let partial = error.partial().unwrap();
    assert!(partial.user.id.is_empty());
    assert!(partial.secret_key().expose_secret() == "issued-secret-canary");
    assert!(
        !serde_json::to_string(partial)
            .unwrap()
            .contains("issued-secret-canary")
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn repository_confirmation_failure_retains_sanitized_partial() {
    let server = Server::start(vec![
        response(
            "202 Accepted",
            r#"{"uuid":"repo-1","name":"synthetic-registry-token","quotaUsed":0.5}"#,
        ),
        response("403 Forbidden", "{}"),
    ]);
    let error = client(&server)
        .create_repository(
            &RequestContext::new().0,
            CreateRepositoryInput::new("app-images", 4),
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotSettled));
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let partial = error.partial().unwrap();
    assert_eq!(partial.repository.id, "repo-1");
    assert_eq!(partial.repository.name, "<redacted>");
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"repoName":"app-images","quotaLimit":4,"isPublic":false})
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn delete_user_confirms_absence_without_replaying_delete() {
    let server = Server::start(vec![
        response("500 Internal Server Error", "{}"),
        response("200 OK", r#"{"listData":[],"totalPage":0,"totalItem":0}"#),
    ]);
    let error = client(&server)
        .delete_user(&RequestContext::new().0, DeleteUserInput::new("robot-1"))
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::NotFound));
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.partial().is_none());
    let heads = server.heads();
    assert!(heads[0].starts_with(b"DELETE /v1/user/robot-1 "));
    assert!(heads[1].starts_with(b"GET /v1/user?name=&size=10000 "));
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn mutation_503_is_never_replayed() {
    let ctx = RequestContext::new().0;
    let server = Server::start(vec![response("503 Service Unavailable", "{}")]);
    let error = client(&server)
        .create_repository(&ctx, CreateRepositoryInput::new("app-images", 4))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.partial().is_none());
    assert_eq!(server.requests(), 1);

    let server = Server::start(vec![
        response("200 OK", r#"{"uuid":"repo-1","imageCount":0}"#),
        response("503 Service Unavailable", "{}"),
    ]);
    let error = client(&server)
        .delete_repository(&ctx, DeleteRepositoryInput::new("repo-1"))
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(error.partial().is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn create_user_validates_later_pages_before_post() {
    let server = Server::start(vec![
        response("200 OK", r#"[{"uuid":"policy-1","action":"pull"}]"#),
        response(
            "200 OK",
            r#"{"data":[{"uuid":"robot-1","name":"somebody"}],"page":1,"totalPage":2,"totalItem":2}"#,
        ),
        response(
            "200 OK",
            r#"{"data":[{"uuid":"robot-2","name":"app-ci"}],"page":2,"totalPage":2,"totalItem":2}"#,
        ),
    ]);
    let error = client(&server)
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["pull".into()])],
            ),
        )
        .await
        .unwrap_err();
    assert!(error.error().has_class(ErrorClass::InvalidInput));
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 3);
    assert!(server.heads()[2].starts_with(b"GET /v1/user?name=app-ci&page=2&size=10000 "));
}

#[derive(Default)]
struct SequenceProvider {
    calls: std::sync::atomic::AtomicUsize,
    fail_at: Option<usize>,
}

impl crate::CredentialsProvider for SequenceProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::Token, Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let count = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.fail_at == Some(count) {
                return Err(Error::provider_failed());
            }
            Ok(crate::Token::new(
                Secret::new(format!("synthetic-bearer-{count}")),
                None,
            ))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn user_preflight_rejects_reflection_of_bearer_from_permission_read() {
    let server = Server::start(vec![
        response("200 OK", r#"[{"uuid":"policy-1","action":"pull"}]"#),
        response(
            "200 OK",
            r#"{"data":[{"uuid":"robot-1","name":"other-user","backendName":"synthetic-bearer-0"}],"totalPage":1,"totalItem":1}"#,
        ),
    ]);
    let client = client_with(
        &server,
        ConfigOptions::default().with_credentials_provider(Arc::new(SequenceProvider::default())),
    );
    let error = client
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["pull".into()])],
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
    assert!(
        server.heads()[1]
            .windows(b"synthetic-bearer-1".len())
            .any(|value| value == b"synthetic-bearer-1")
    );
}

#[tokio::test]
async fn create_user_keeps_authentication_error_from_name_preflight() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"[{"uuid":"policy-1","action":"pull"}]"#,
    )]);
    let provider = SequenceProvider {
        fail_at: Some(1),
        ..SequenceProvider::default()
    };
    let client = client_with(
        &server,
        ConfigOptions::default().with_credentials_provider(Arc::new(provider)),
    );
    let failure = client
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["pull".into()])],
            ),
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::Auth));
    assert!(failure.error().api_error().is_none());
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn sensitive_preflight_distinguishes_invalid_json_and_shape() {
    for (body, message) in [
        ("[", "invalid JSON response; body withheld"),
        ("{}", "response failed to decode; body withheld"),
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let failure = client(&server)
            .create_user(
                &RequestContext::new().0,
                CreateUserInput::new(
                    "app-ci",
                    vec![UserPermission::new("repo-1", vec!["pull".into()])],
                ),
            )
            .await
            .unwrap_err();
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.message(), message);
        assert_eq!(api.operation(), "containerregistry.CreateUser");
        assert_eq!(api.status(), None);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn unknown_action_diagnostic_withholds_a_reassembled_bearer() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"[{"uuid":"policy-1","action":"synthetic-registry-\u0007token"}]"#,
    )]);
    let failure = client(&server)
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["unknown".into()])],
            ),
        )
        .await
        .unwrap_err();
    let message = failure.error().api_error().unwrap().message();
    assert!(!message.contains("synthetic-registry-token"));
    assert!(message.contains("unknown action"));
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn failed_lookup_keeps_private_delivery_outside_the_error() {
    let server = Server::start(vec![
        response("200 OK", r#"[{"uuid":"policy-1","action":"pull"}]"#),
        response("200 OK", r#"{"data":[],"totalPage":0,"totalItem":0}"#),
        response("200 OK", r#"{"secretKey":"issued-secret-canary"}"#),
    ]);
    let provider = SequenceProvider {
        fail_at: Some(3),
        ..SequenceProvider::default()
    };
    let client = client_with(
        &server,
        ConfigOptions::default().with_credentials_provider(Arc::new(provider)),
    );
    let failure = client
        .create_user(
            &RequestContext::new().0,
            CreateUserInput::new(
                "app-ci",
                vec![UserPermission::new("repo-1", vec!["pull".into()])],
            ),
        )
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert!(
        issued_user_secret(&failure)
            .is_some_and(|key| key.expose_secret() == "issued-secret-canary")
    );
    let (error, partial, dispatch) = failure.into_parts();
    assert_eq!(
        error.detail::<ErrorDetail>(),
        Some(&ErrorDetail::UserNotFound)
    );
    assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
    assert!(issued_user_secret(&Failure::new(error, partial, dispatch)).is_none());
    assert_eq!(server.requests(), 3);
}
