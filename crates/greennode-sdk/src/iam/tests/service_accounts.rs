use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{client, response};
use crate::iam::{
    Client, CreateServiceAccountInput, DecodeError, DeleteServiceAccountInput,
    UpdateServiceAccountInput, WriteError, failed_service_account_cleanup,
    issued_service_account_secret, redact_failed_service_account_create,
};
use crate::wire::test_server::Server;
use crate::{
    Config, ConfigOptions, CredentialsProvider, EndpointOverrides, Error, ErrorClass,
    RequestContext, Secret, Token, WriteDispatch,
};

const CONFIRMATION_BEARER: &str = "toy-confirmation-bearer";

#[derive(Default)]
struct ChangingProvider(AtomicUsize);

impl CredentialsProvider for ChangingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let bearer = if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            "toy-create-bearer"
        } else {
            CONFIRMATION_BEARER
        };
        Box::pin(async move { Ok(Token::new(Secret::new(bearer), None)) })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn failed_confirmation_screens_private_file_path_with_fresh_bearer() {
    let server = Server::start(vec![
        response(
            "201 Created",
            br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
        ),
        response("404 Not Found", b"{}"),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_credentials_provider(Arc::new(ChangingProvider::default()))
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
    let failure = Client::new(&config)
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 2);
    let path = format!("private/{CONFIRMATION_BEARER}/issued");
    assert!(redact_failed_service_account_create(&failure, &path) == "<redacted>");
    assert_eq!(
        redact_failed_service_account_create(&failure, "private/issued"),
        "private/issued"
    );
    assert!(
        issued_service_account_secret(&failure)
            .is_some_and(|secret| secret.expose_secret() == "toy-issued-key")
    );
    let partial = failure.partial().unwrap();
    assert!(partial.client_secret().is_none());
    assert!(!format!("{partial:?}").contains("toy-issued-key"));
}

#[tokio::test]
async fn cleanup_keeps_the_original_created_identity() {
    let server = Server::start(vec![
        response(
            "201 Created",
            br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
        ),
        response("200 OK", br#"{"id":"sa-1"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut output = client(&server, "toy-bearer")
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap();
    output.service_account.id = "sa-2".into();
    assert_eq!(output.cleanup_input().unwrap().service_account_id, "sa-1");
}

#[tokio::test]
async fn account_confirmation_never_follows_redirects_reflecting_the_issued_secret() {
    for (location, final_path, confirmed) in [
        (
            "/dashboard/accounts-api/v1/service-accounts/confirmed?value=public",
            "/dashboard/accounts-api/v1/service-accounts/confirmed?value=public",
            false,
        ),
        (
            "/dashboard/accounts-api/v1/service-accounts/confirmed?value=%74oy-issued-key",
            "/dashboard/accounts-api/v1/service-accounts/sa-1",
            true,
        ),
        (
            "/dashboard/accounts-api/v1/service-accounts/confirmed?value=%74oy-issued-key",
            "/dashboard/accounts-api/v1/service-accounts/sa-1",
            false,
        ),
    ] {
        let server = Server::start(vec![
            response(
                "201 Created",
                br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
            ),
            crate::wire::test_server::Response::Http {
                status: "307 Temporary Redirect".into(),
                headers: vec![
                    ("Location".into(), location.into()),
                    ("Content-Length".into(), "0".into()),
                ],
                body: Vec::new(),
            },
            if confirmed || location == final_path {
                response("200 OK", br#"{"id":"sa-1"}"#)
            } else {
                response("403 Forbidden", b"{}")
            },
        ]);
        let (ctx, _) = RequestContext::new();
        let result = client(&server, "toy-bearer")
            .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
            .await;
        assert_eq!(result.is_ok(), confirmed);
        if let Err(failure) = result {
            assert_eq!(
                failure.error().detail::<WriteError>(),
                Some(&WriteError::CreateUnconfirmed)
            );
            assert!(issued_service_account_secret(&failure).is_some());
        }
        assert_eq!(server.requests(), 3);
        assert!(server.heads()[2].starts_with(format!("GET {final_path} ").as_bytes()));
    }
}

#[tokio::test]
async fn create_with_unusable_path_id_retains_only_the_private_secret() {
    for (bearer, body) in [
        (
            "toy-bearer",
            br#"{"id":"sa_1","clientSecret":"toy-issued-key"}"#.as_slice(),
        ),
        (
            "a?bcd",
            br#"{"id":"YT9iY2Q","clientSecret":"toy-issued-key"}"#.as_slice(),
        ),
    ] {
        let server = Server::start(vec![response("201 Created", body)]);
        let (ctx, _) = RequestContext::new();
        let failure = client(&server, bearer)
            .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
            .await
            .unwrap_err();
        assert_eq!(server.requests(), 1);
        assert_eq!(
            failure.error().detail::<WriteError>(),
            Some(&WriteError::CreateUnconfirmed)
        );
        assert!(failure.partial().unwrap().service_account.id.is_empty());
        assert!(failed_service_account_cleanup(&failure).is_none());
        assert!(issued_service_account_secret(&failure).is_some());
    }
}

fn account_guard() -> Vec<crate::wire::test_server::Response> {
    vec![
        response("200 OK", br#"{"userId":"user-1","userType":"iam-user"}"#),
        response("200 OK", br#"[{"action":"CreatePolicy","label":"Write"}]"#),
        response("200 OK", br#"{"data":[],"totalItems":0}"#),
    ]
}

#[tokio::test]
async fn reflected_read_back_does_not_confirm_create_or_update() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![
        response(
            "201 Created",
            br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
        ),
        response(
            "200 OK",
            br#"{"id":"sa-1","description":"%74oy-issued-key"}"#,
        ),
    ]);
    let failure = client(&server, "toy-bearer")
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteError>(),
        Some(&WriteError::CreateUnconfirmed)
    );
    assert!(issued_service_account_secret(&failure).is_some());
    let account = &failure.partial().unwrap().service_account;
    assert_eq!(account.id, "sa-1");
    assert!(account.description.is_empty());

    let mut replies = account_guard();
    replies.extend([
        response("200 OK", b""),
        response("200 OK", br#"{"id":"sa-1","description":"toy-bearer"}"#),
    ]);
    let server = Server::start(replies);
    let failure = client(&server, "toy-bearer")
        .update_service_account(&ctx, UpdateServiceAccountInput::new("sa-1"))
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    let account = &failure.partial().unwrap().service_account;
    assert_eq!(account.id, "sa-1");
    assert!(account.description.is_empty());
}

#[tokio::test]
async fn rejected_account_writes_and_guards_use_only_the_status_and_fixed_message() {
    const PEER: &[u8] = br#"{"code":"PeerCode","message":"peer diagnostic"}"#;
    let (ctx, _) = RequestContext::new();
    for command in ["patch", "delete", "guard"] {
        let mut replies = if command == "guard" {
            Vec::new()
        } else {
            account_guard()
        };
        replies.push(response("403 Forbidden", PEER));
        let server = Server::start(replies);
        let client = client(&server, "toy-bearer");
        let error = if command == "patch" {
            client
                .update_service_account(&ctx, UpdateServiceAccountInput::new("sa-1"))
                .await
                .unwrap_err()
                .into_parts()
                .0
        } else {
            client
                .delete_service_account(&ctx, DeleteServiceAccountInput::new("sa-1"))
                .await
                .unwrap_err()
                .into_parts()
                .0
        };
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(403));
        assert_eq!(api.code(), "Forbidden");
        assert!(
            api.message()
                .contains("IAM user request failed; response withheld")
        );
        assert!(!format!("{error:?}").contains("peer diagnostic"));
        assert_eq!(server.requests(), if command == "guard" { 1 } else { 4 });
    }
}

#[tokio::test]
async fn empty_create_response_is_ambiguous_and_missing_secret_keeps_unconfirmed_identity() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response("201 Created", b"")]);
    let failure = client(&server, "toy-bearer")
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert_eq!(
        failure.error().detail::<DecodeError>(),
        Some(&DecodeError::MalformedJson)
    );
    assert!(failure.partial().is_none());
    assert!(issued_service_account_secret(&failure).is_none());
    assert_eq!(server.requests(), 1);

    let server = Server::start(vec![
        response("201 Created", br#"{"id":"sa-1"}"#),
        response("404 Not Found", b"{}"),
    ]);
    let failure = client(&server, "toy-bearer")
        .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<WriteError>(),
        Some(&WriteError::CreateUnconfirmed)
    );
    assert_eq!(failure.partial().unwrap().service_account.id, "sa-1");
    assert!(issued_service_account_secret(&failure).is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn account_confirmation_ignores_unknown_out_of_range_numbers() {
    for (body, safe) in [
        (br#"{"id":"sa-1","unknown":1e1000}"#.as_slice(), true),
        (
            br#"{"id":"sa-1","unknown":[1e1000,"%74oy-issued-key"]}"#.as_slice(),
            false,
        ),
    ] {
        let server = Server::start(vec![
            response(
                "201 Created",
                br#"{"id":"sa-1","clientSecret":"toy-issued-key"}"#,
            ),
            response("200 OK", body),
        ]);
        let (ctx, _) = RequestContext::new();
        let result = client(&server, "toy-bearer")
            .create_service_account(&ctx, CreateServiceAccountInput::new("worker"))
            .await;
        if safe {
            assert_eq!(result.unwrap().service_account.id, "sa-1");
        } else {
            let failure = result.unwrap_err();
            assert_eq!(
                failure.error().detail::<WriteError>(),
                Some(&WriteError::CreateUnconfirmed)
            );
            assert!(issued_service_account_secret(&failure).is_some());
        }
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn service_account_absence_requires_the_dispatched_delete() {
    for dispatched in [false, true] {
        let mut replies = vec![
            response("200 OK", br#"{"userId":"user-1","userType":"iam-user"}"#),
            response("200 OK", br#"[{"action":"CreatePolicy","label":"Write"}]"#),
        ];
        if dispatched {
            replies.push(response("200 OK", br#"{"data":[],"totalItems":0}"#));
        }
        replies.push(response("404 Not Found", b"{}"));
        let server = Server::start(replies);
        let failure = client(&server, "toy-bearer")
            .delete_service_account(
                &RequestContext::new().0,
                DeleteServiceAccountInput::new("sa-1"),
            )
            .await
            .unwrap_err();
        assert_eq!(
            failure.write_dispatch(),
            if dispatched {
                WriteDispatch::MayHaveBeenSent
            } else {
                WriteDispatch::NotSent
            }
        );
        assert_eq!(server.requests(), if dispatched { 4 } else { 3 });
        assert_eq!(
            crate::confirms_absence(
                failure.error(),
                "iam.DeleteServiceAccount",
                "DELETE",
                "accounts-api/v1/service-accounts/{}"
            ),
            dispatched
        );
        assert_eq!(
            crate::confirms_absence(
                failure.error(),
                "iam.DeleteServiceAccount",
                "GET",
                "policies-api/v1/user-attachments/service-accounts/{}/policies"
            ),
            !dispatched
        );
    }
}

#[tokio::test]
async fn account_diagnostic_translation_preserves_clean_absence() {
    for (bearer, status) in [("toy-bearer", Some(404)), ("404", None)] {
        let server = Server::start(vec![response("404 Not Found", b"{}")]);
        let failure = client(&server, bearer)
            .get_service_account(
                &RequestContext::new().0,
                crate::iam::GetServiceAccountInput::new("sa-1"),
            )
            .await
            .unwrap_err();
        assert!(crate::confirms_absence(
            failure.error(),
            "iam.GetServiceAccount",
            "GET",
            "accounts-api/v1/service-accounts/{}"
        ));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), status);
        if status.is_none() {
            assert!(api.message() == "IAM response withheld");
        }
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn redirected_account_404_cannot_prove_absence_at_the_requested_route() {
    let server = Server::start(vec![
        crate::wire::test_server::Response::Http {
            status: "307 Temporary Redirect".into(),
            headers: vec![
                (
                    "Location".into(),
                    "/dashboard/accounts-api/v1/service-accounts/sa-2".into(),
                ),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response("404 Not Found", b"{}"),
    ]);
    let failure = client(&server, "toy-bearer")
        .get_service_account(
            &RequestContext::new().0,
            crate::iam::GetServiceAccountInput::new("sa-1"),
        )
        .await
        .unwrap_err();
    assert!(!crate::confirms_absence(
        failure.error(),
        "iam.GetServiceAccount",
        "GET",
        "accounts-api/v1/service-accounts/{}"
    ));
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn unclean_acknowledgement_never_issues_a_reflecting_or_ambiguous_secret() {
    for body in [
        br#"{"id":"toy-bearer","clientSecret":"toy%2Dbearer"}"#.as_slice(),
        br#"{"id":"toy-bearer","clientSecret":"first","clientSecret":"second"}"#.as_slice(),
        br#"{"id":"toy-bearer","clientSecret":"first","ClientSecret":"second"}"#.as_slice(),
    ] {
        let server = Server::start(vec![response("201 Created", body)]);
        let failure = client(&server, "toy-bearer")
            .create_service_account(
                &RequestContext::new().0,
                CreateServiceAccountInput::new("worker"),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert!(issued_service_account_secret(&failure).is_none());
        assert!(failed_service_account_cleanup(&failure).is_none());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn reset_private_delivery_only_extracts_the_declared_secret() {
    let mut replies = account_guard();
    replies.push(response(
        "200 OK",
        br#"{"clientSecret":"clientSecret-is-valid","unknown":"toy-bearer"}"#,
    ));
    let server = Server::start(replies);
    let failure = client(&server, "toy-bearer")
        .reset_service_account_secret(
            &RequestContext::new().0,
            crate::iam::ResetServiceAccountSecretInput::new("sa-1"),
        )
        .await
        .unwrap_err();
    assert!(failure.error().has_identity(ErrorClass::NotSettled));
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    assert!(
        crate::iam::issued_reset_service_account_secret(&failure)
            .is_some_and(|secret| secret.expose_secret() == "clientSecret-is-valid")
    );
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn field_only_issuance_reads_an_escaped_secret_key() {
    let server = Server::start(vec![response(
        "201 Created",
        br#"{"id":"toy-bearer","clientSecr\u0065t":"issued-secret"}"#,
    )]);
    let failure = client(&server, "toy-bearer")
        .create_service_account(
            &RequestContext::new().0,
            CreateServiceAccountInput::new("worker"),
        )
        .await
        .unwrap_err();
    assert!(
        issued_service_account_secret(&failure)
            .is_some_and(|secret| secret.expose_secret() == "issued-secret")
    );
    assert!(failed_service_account_cleanup(&failure).is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn private_issuance_preserves_secret_when_other_fields_expand_during_normalization() {
    let body = b"{\"id\":\"toy-bearer\",\"clientSecret\":\"issued-secret\",\"unknown\":\"\xff\"}";
    let server = Server::start(vec![response("201 Created", body)]);
    let failure = client(&server, "toy-bearer")
        .create_service_account(
            &RequestContext::new().0,
            CreateServiceAccountInput::new("worker"),
        )
        .await
        .unwrap_err();
    assert!(
        issued_service_account_secret(&failure)
            .is_some_and(|secret| secret.expose_secret() == "issued-secret")
    );
    assert_eq!(server.requests(), 1);
}
