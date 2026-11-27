use super::{client, response};
use crate::iam::{Client, CreateUserInput, WriteError, generated_create_user_password};
use crate::wire::test_server::Server;
use crate::{
    Config, ConfigOptions, EndpointOverrides, ErrorClass, IamUserAuth, RequestContext, Secret,
    WriteDispatch,
};

#[tokio::test]
async fn create_refuses_password_equal_to_configured_iam_login_password() {
    let server = Server::start(Vec::new());
    let password = Secret::new("toy-configured-password");
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_iam_user(IamUserAuth::new(
                "toy@example.test".into(),
                "login-user".into(),
                password.clone(),
            ))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let mut input = CreateUserInput::new("worker");
    input.password = Some(password);
    let failure = Client::new(&config)
        .create_user(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(failure.error().has_identity(ErrorClass::InvalidInput));
    assert!(generated_create_user_password(&failure).is_none());
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn uncertain_create_keeps_password_private_and_refuses_reflected_identity() {
    let server = Server::start(vec![response(
        "201 Created",
        br#"{"id":"prefixYT9iY2Qsuffix"}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "a?bcd")
        .create_user(&ctx, CreateUserInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteError>(),
        Some(&WriteError::UserCreateUncertain)
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let output = failure.partial().unwrap();
    assert!(output.created_id.is_empty() && output.user.id.is_empty());
    assert!(output.generated_password().is_none());
    let password = generated_create_user_password(&failure).unwrap();
    assert_eq!(password.expose_secret().len(), 32);
    assert!(!format!("{failure:?} {output:?}").contains(password.expose_secret()));
    assert_eq!(server.requests(), 1);
}

#[test]
fn debug_redacts_password_and_reflected_operands() {
    let password = Secret::new("toy-private-password");
    let mut create = CreateUserInput::new(password.expose_secret());
    create.password = Some(password.clone());
    let mut reset = crate::iam::ResetUserPasswordInput::new(password.expose_secret());
    reset.password = Some(password.clone());
    assert!(!format!("{create:?} {reset:?}").contains(password.expose_secret()));
    create.username = "safe-user".to_owned();
    reset.user_id = "safe-resource".to_owned();
    assert!(format!("{create:?}").contains("safe-user"));
    assert!(format!("{reset:?}").contains("safe-resource"));
}

const CALLER: &[u8] = br#"{"userId":"caller-1","userType":"iam-user"}"#;
const MFA_BEFORE: &[u8] = br#"{"id":"user-1","from":"iam","twoFactorAuth":{"googleAuthenticator":{"verified":true,"enabled":false}}}"#;
const MFA_AFTER: &[u8] = br#"{"id":"user-1","from":"iam","twoFactorAuth":{"googleAuthenticator":{"verified":true,"enabled":true}}}"#;

fn mfa_guard() -> Vec<crate::wire::test_server::Response> {
    vec![
        response("200 OK", CALLER),
        response("200 OK", MFA_BEFORE),
        response("200 OK", br#"[{"action":"CreatePolicy","label":"Write"}]"#),
        response("200 OK", br#"{"data":[],"totalItems":0}"#),
        response("200 OK", b"[]"),
    ]
}

#[tokio::test]
async fn mfa_checks_observed_state_even_when_processing_204_fails() {
    let mut replies = mfa_guard();
    replies.push(crate::wire::test_server::Response::Http {
        status: "204 No Content".into(),
        headers: vec![("Content-Encoding".into(), "unsupported".into())],
        body: Vec::new(),
    });
    replies.push(response("200 OK", MFA_AFTER));
    let server = Server::start(replies);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, "toy-bearer")
        .enable_user_mfa(
            &ctx,
            crate::iam::EnableUserMfaInput::new("user-1", "google"),
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(
        failure.partial().map(|out| (out.enabled, out.changed)),
        Some((Some(true), true))
    );
    assert_eq!(server.requests(), 7);
}

#[tokio::test]
async fn uncertain_create_retains_the_lookup_http_cause() {
    let server = Server::start(vec![
        response("201 Created", b""),
        response("404 Not Found", br#"{"message":"peer canary"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let failed = client(&server, "toy-bearer")
        .create_user(&ctx, CreateUserInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(
        failed.error().detail::<WriteError>(),
        Some(&WriteError::UserCreateUncertain)
    );
    let api = failed.error().api_error().unwrap();
    assert_eq!(api.status(), Some(404));
    assert!(
        api.message()
            .contains("IAM user request failed; response withheld")
    );
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn cancelled_target_read_keeps_cancellation_under_origin_refusal() {
    let (notify, requested) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![
        response("200 OK", CALLER),
        crate::wire::test_server::Response::StallBeforeHead(notify),
    ]);
    let (ctx, cancel) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let (failed, ()) = tokio::join!(
        client.delete_user(&ctx, crate::iam::DeleteUserInput::new("user-1")),
        async {
            requested.await.unwrap();
            cancel.cancel();
        },
    );
    let failed = failed.unwrap_err();
    assert_eq!(
        failed.error().detail::<WriteError>(),
        Some(&WriteError::OriginNotIam)
    );
    assert!(failed.error().has_identity(ErrorClass::Cancelled));
    assert_eq!(failed.write_dispatch(), WriteDispatch::NotSent);
    assert_eq!(server.requests(), 2);
}

#[test]
fn private_http_error_retains_cancellation_after_observed_rejection_headers() {
    use super::super::{user_writes::user_write, wire::private_status_error};
    use crate::error::SafeText;
    use crate::wire::product::{evidence::Evidence, redirect::Retry};
    use crate::{ApiError, Error};
    use http::{Method, StatusCode};

    let op = user_write(
        "iam.CreateUser",
        Method::POST,
        "accounts-api/v1/iam-users",
        Retry::Once,
        &[StatusCode::CREATED],
    );
    let error = Error::new(ErrorClass::Cancelled)
        .with_cause(Error::from_api(ApiError::without_fallback(
            op.id,
            Some(404),
            SafeText::new("", &[]),
            SafeText::new("", &[]),
            false,
        )))
        .with_outer_detail(StatusCode::NOT_FOUND);
    let error = private_status_error(&op, error, &Evidence::new());
    assert_eq!(error.api_error().and_then(|api| api.status()), Some(404));
    assert!(error.has_identity(ErrorClass::Cancelled));
}

const TOY_PASSWORD: &str = "Toy-pass9";
const HEX_PASSWORD: &str = "546f792d7061737339";

#[tokio::test]
async fn create_rejects_username_reflecting_hex_encoded_password() {
    let server = Server::start(Vec::new());
    let (ctx, _) = RequestContext::new();
    let mut input = CreateUserInput::new(HEX_PASSWORD);
    input.password = Some(Secret::new(TOY_PASSWORD));
    let failed = client(&server, "toy-bearer")
        .create_user(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(failed.write_dispatch(), WriteDispatch::NotSent);
    assert!(failed.error().has_identity(ErrorClass::InvalidInput));
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn create_refuses_to_confirm_or_publish_created_id_reflecting_hex_encoded_password() {
    let body = format!(r#"{{"id":"id-{HEX_PASSWORD}"}}"#);
    let server = Server::start(vec![
        response("201 Created", body.as_bytes()),
        response("404 Not Found", b"{}"),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateUserInput::new("worker");
    input.password = Some(Secret::new(TOY_PASSWORD));
    let failed = client(&server, "toy-bearer")
        .create_user(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failed.error().detail::<WriteError>(),
        Some(&WriteError::UserCreateUncertain)
    );
    let partial = failed.partial().unwrap();
    assert!(partial.user.id.is_empty() && partial.created_id.is_empty());
    assert!(
        !serde_json::to_string(partial)
            .unwrap()
            .contains(HEX_PASSWORD)
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn create_refuses_confirmation_redirect_reflecting_hex_encoded_password() {
    let server = Server::start(vec![
        response("201 Created", br#"{"id":"user-1"}"#),
        crate::wire::test_server::Response::Http {
            status: "307 Temporary Redirect".into(),
            headers: vec![
                (
                    "Location".into(),
                    format!("/dashboard/accounts-api/v1/iam-users/user-1?ref={HEX_PASSWORD}"),
                ),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response(
            "200 OK",
            br#"{"id":"user-1","username":"worker","from":"iam"}"#,
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = CreateUserInput::new("worker");
    input.password = Some(Secret::new(TOY_PASSWORD));
    let failed = client(&server, "toy-bearer")
        .create_user(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(
        failed.error().detail::<WriteError>(),
        Some(&WriteError::CreateUnconfirmed)
    );
    assert_eq!(server.requests(), 2);
}

struct ConfirmationPasswordProvider(std::sync::Arc<Server>);

impl crate::CredentialsProvider for ConfirmationPasswordProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::Token, crate::Error>> + Send + 'a>,
    > {
        Box::pin(async move {
            let body = self.0.bodies();
            let token = match body.first() {
                None => Secret::new("toy-initial-token"),
                Some(request) => {
                    let fields: serde_json::Value = serde_json::from_slice(request).unwrap();
                    Secret::new(fields["password"].as_str().unwrap())
                }
            };
            Ok(crate::Token::new(token, None))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), crate::Error>> + Send + 'a>>
    {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn confirmed_create_withholds_password_reused_as_the_confirmation_bearer() {
    let server = std::sync::Arc::new(Server::start(vec![
        response("201 Created", br#"{"id":"user-1"}"#),
        response(
            "200 OK",
            br#"{"id":"user-1","username":"worker","from":"iam"}"#,
        ),
    ]));
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_project_id("unscoped".into())
            .with_credentials_provider(std::sync::Arc::new(ConfirmationPasswordProvider(
                server.clone(),
            )))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
                iam: Some(format!("{}/iam/", server.origin)),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let output = Client::new(&config)
        .create_user(&RequestContext::new().0, CreateUserInput::new("worker"))
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(output.created_id, "user-1");
    assert!(output.generated_password().is_none());
}
