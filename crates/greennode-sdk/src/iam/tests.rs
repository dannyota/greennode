mod groups;
mod service_accounts;
mod users;

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

fn response(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn client(server: &Server, bearer: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("han-1".into())
            .with_project_id("unscoped".into())
            .with_static_token(Secret::new(bearer))
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
    Client::new(&config)
}

#[tokio::test]
async fn baseline_user_fixtures_keep_only_supported_mfa_states() {
    let server = Server::start(vec![
        response(
            "200 OK",
            include_bytes!("../../tests/fixtures/iam/get_user.json"),
        ),
        response(
            "200 OK",
            include_bytes!("../../tests/fixtures/iam/get_user_mfa.json"),
        ),
        response(
            "200 OK",
            include_bytes!("../../tests/fixtures/iam/list_users_filtered.json"),
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let id = "00000000-0000-4000-8000-000000000001";
    for last_use in [0, 1_760_000_100_000] {
        let user = client
            .get_user(&ctx, GetUserInput::new(id))
            .await
            .unwrap()
            .user;
        assert_eq!(user.last_use, last_use);
        let mfa = serde_json::to_value(user.mfa).unwrap();
        assert_eq!(
            mfa,
            serde_json::json!({
                "Email":{"Verified":false,"Enabled":false},
                "GoogleAuthenticator":{"Verified":false,"Enabled":false}
            })
        );
    }
    let page = client
        .list_users(
            &ctx,
            ListUsersInput {
                name: "probe".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.page_number, Some(0));
    assert_eq!(page.items[0].id, id);
    assert!(server.heads()[2].starts_with(
        b"GET /dashboard/accounts-api/v1/iam-users?name=probe&pageNumber=0&pageSize=10000 "
    ));
}

#[tokio::test]
async fn page_request_and_optional_metadata_match_iam() {
    let body = br#"{"data":[{"id":"user_1","username":"a","createdAt":"now"}],"pageNumber":-0,"pageSize":null}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (ctx, _) = RequestContext::new();
    let user_page = client(&server, "toy-bearer")
        .list_users(
            &ctx,
            ListUsersInput {
                name: "a+b".into(),
                page: -1,
                size: 7,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (
            user_page.page_number,
            user_page.page_size,
            user_page.total_items,
            user_page.total_pages
        ),
        (Some(0), None, None, None)
    );
    assert_eq!(user_page.items[0].id, "user_1");
    assert!(server.heads()[0].starts_with(
        b"GET /dashboard/accounts-api/v1/iam-users?name=a%2Bb&pageNumber=0&pageSize=7 "
    ));
}

#[tokio::test]
async fn policy_conditions_screen_reflected_keys_values_and_rendered_numbers() {
    let body = br#"{"id":"policy-1","createdAt":42,"statements":[{"effect":"allow","condition":{"stringEquals":{"42-key":"keep","keep":"42-value","number":4.2e1,"safe":"public"}}}]}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, "42")
        .get_policy(&ctx, GetPolicyInput::new("policy-1"))
        .await
        .unwrap();
    let serialized = serde_json::to_value(output).unwrap();
    assert!(serialized["Policy"]["CreatedAt"] == "<redacted>");
    let condition = &serialized["Policy"]["Statements"][0]["Condition"]["stringEquals"];
    assert!(
        condition
            == &serde_json::json!({"keep":"<redacted>","number":"<redacted>","safe":"public"})
    );
    assert!(server.heads()[0].starts_with(b"GET /iam/policies-api/v1/policies/policy-1 "));
}

#[tokio::test]
async fn user_decodes_go_strings_and_validates_types_without_error_details() {
    let replacements = b"{\"username\":\"a\xed\xa0\x80\\ud800\",\"createdAt\":\"1969-12-31T23:59:59.9999Z\",\"lastUse\":-0}";
    let server = Server::start(vec![
        response("200 OK", replacements),
        response(
            "200 OK",
            br#"{"username": {"$serde_json::private::Number": "1"}}"#,
        ),
        response(
            "200 OK",
            br#"{"twoFactorAuth":{"Email":{"enabled":true,"Enabled":false}}}"#,
        ),
        response("200 OK", b"{\"username\":"),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let input = GetUserInput::new("user-1");
    let user = client.get_user(&ctx, input.clone()).await.unwrap().user;
    assert_eq!(user.username, "a����");
    assert_eq!((user.created_at, user.last_use), (-1, 0));
    for detail in [
        DecodeError::InvalidShape,
        DecodeError::InvalidShape,
        DecodeError::MalformedJson,
    ] {
        let failure = client.get_user(&ctx, input.clone()).await.unwrap_err();
        let (error, partial, dispatch) = failure.into_parts();
        assert_eq!(error.detail::<DecodeError>(), Some(&detail));
        assert_eq!(error.api_error().unwrap().status(), None);
        assert_eq!(
            error.api_error().unwrap().message(),
            "response failed to decode; body withheld"
        );
        assert!(partial.is_none());
        assert_eq!(dispatch, WriteDispatch::NotAWrite);
    }
}

#[tokio::test]
async fn numeric_credential_is_omitted_from_status_and_withheld_message() {
    let server = Server::start(vec![response("403 Forbidden", br#"{"message":"denied"}"#)]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, "403")
        .get_service_account(&ctx, GetServiceAccountInput::new("sa-1"))
        .await
        .unwrap_err();
    let error = result.into_parts().0;
    assert!(error.has_class(ErrorClass::PermissionDenied));
    let api = error.api_error().unwrap();
    assert_eq!(api.code(), "Forbidden");
    assert!(api.status().is_none());
    assert!(api.message() == "IAM response withheld");
}

#[tokio::test]
async fn oversized_service_account_responses_keep_go_error_precedence() {
    let oversized = vec![b' '; OBJECT_CAP + 1];
    let server = Server::start(vec![
        response("200 OK", &oversized),
        response("403 Forbidden", &oversized),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let input = GetServiceAccountInput::new("sa-1");
    let error = client
        .get_service_account(&ctx, input.clone())
        .await
        .unwrap_err()
        .into_parts()
        .0;
    assert!(error.is_body_too_large());
    let api = error.api_error().unwrap();
    assert!(api.status().is_none());
    assert!(api.operation().is_empty());
    assert!(
        api.message()
            == "iam.GetServiceAccount: read service account: transport: response body exceeds limit"
    );
    let error = client
        .get_service_account(&ctx, input)
        .await
        .unwrap_err()
        .into_parts()
        .0;
    let api = error.api_error().unwrap();
    assert_eq!(api.status(), Some(403));
    assert!(
        api.message()
            .contains("IAM user request failed; response withheld")
    );
}

#[tokio::test]
async fn attachment_failure_discards_previous_results_and_stops_before_third_read() {
    let server = Server::start(vec![
        response("200 OK", br#"[{"id":"group-1","name":"public"}]"#),
        response("403 Forbidden", br#"{"message":"denied"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, "toy-bearer")
        .list_policy_attachments(&ctx, ListPolicyAttachmentsInput::new("policy-1"))
        .await
        .unwrap_err();
    let (error, partial, dispatch) = result.into_parts();
    assert_eq!(error.api_error().unwrap().status(), Some(403));
    assert!(partial.is_none());
    assert_eq!(dispatch, WriteDispatch::NotAWrite);
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn user_fields_decode_exact_case_and_reject_folded_duplicates() {
    let server = Server::start(vec![
        response(
            "200 OK",
            br#"{"id":"user-1","USERNAME":"ignored","TWOFACTORAUTH":false}"#,
        ),
        response("200 OK", br#"{"username":"known","USERNAME":"duplicate"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let input = GetUserInput::new("user-1");
    assert!(
        client
            .get_user(&ctx, input.clone())
            .await
            .unwrap()
            .user
            .username
            .is_empty()
    );
    assert_eq!(
        client
            .get_user(&ctx, input)
            .await
            .unwrap_err()
            .into_parts()
            .0
            .detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
}

#[tokio::test]
async fn timestamp_formats_match_go() {
    let server = Server::start(vec![
        response("200 OK", br#"{"createdAt":"1970-01-01 00:00:00Z"}"#),
        response(
            "200 OK",
            br#"{"createdAt":"1970-01-01T00:00:00,125123456789Z"}"#,
        ),
        response("200 OK", br#"{"createdAt":"1970-01-02T0:00:00+24:60"}"#),
        response("200 OK", br#"{"createdAt":"1970-01-01T23:59:60Z"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, "toy-bearer");
    let input = GetUserInput::new("user-1");
    let error = client
        .get_user(&ctx, input.clone())
        .await
        .unwrap_err()
        .into_parts()
        .0;
    assert_eq!(
        error.detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
    for expected in [125, -3_600_000] {
        let result = client.get_user(&ctx, input.clone()).await;
        assert!(
            result
                .as_ref()
                .is_ok_and(|output| output.user.created_at == expected)
        );
    }
    let error = client
        .get_user(&ctx, input)
        .await
        .unwrap_err()
        .into_parts()
        .0;
    assert_eq!(
        error.detail::<DecodeError>(),
        Some(&DecodeError::InvalidShape)
    );
}

#[tokio::test]
async fn reflected_observed_status_is_removed_from_typed_details() {
    let oversized = vec![b' '; OBJECT_CAP + 1];
    for (bearer, keeps_status) in [("200", false), ("toy-bearer", true)] {
        let server = Server::start(vec![response("200 OK", &oversized)]);
        let (ctx, _) = RequestContext::new();
        let error = client(&server, bearer)
            .get_user(&ctx, GetUserInput::new("user-1"))
            .await
            .unwrap_err()
            .into_parts()
            .0;
        assert_eq!(error.class(), ErrorClass::Api);
        assert!(error.is_body_too_large());
        assert_eq!(error.detail::<StatusCode>().is_some(), keeps_status);
    }
}
