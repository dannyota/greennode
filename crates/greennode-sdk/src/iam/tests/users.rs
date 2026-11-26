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
