use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

const USER: &[u8] = include_bytes!("../../tests/fixtures/portal/get_user_info.json");
const BEARER: &str = "synthetic-portal-bearer";

#[tokio::test]
async fn quota_refuses_a_credential_in_a_normalized_number() {
    let server = Server::start(vec![response("200 OK", br#"{"quotas":[{"used":1e6}]}"#)]);
    let output = client_with_bearer(&server, "1000000")
        .list_quota_used(&RequestContext::new().0, ListQuotaUsedInput::default())
        .await;
    assert_eq!(server.requests(), 1);
    assert!(output.is_err());
}

#[tokio::test]
async fn sdk_serialization_redacts_nested_dynamic_keys() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"{"data":{"nested":[{"apiToken":"canary","visible":"yes"}]}}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap();
    let value = serde_json::to_value(output).unwrap();
    assert_eq!(value["UserInfo"]["nested"][0]["apiToken"], "<redacted>");
    assert_eq!(value["UserInfo"]["nested"][0]["visible"], "yes");
}

#[tokio::test]
async fn list_metadata_rejects_invalid_values() {
    let (ctx, _) = RequestContext::new();
    for body in [
        br#"{"zones":[],"page":-1}"#.as_slice(),
        br#"{"zones":[],"page":null}"#,
        br#"{"zones":[],"size":1,"pageSize":2}"#,
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let error = client(&server)
            .list_zones(&ctx, ListZonesInput::default())
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    }
}

#[test]
fn failed_refresh_omits_status_but_preserves_auth() {
    let error = Error::new(ErrorClass::Auth).with_http_status(StatusCode::UNAUTHORIZED);
    let error = withheld(&user_info_operation(), error);
    assert!(error.has_class(ErrorClass::Auth));
    assert_eq!(error.api_error().unwrap().status(), None);
    assert_eq!(
        error.api_error().unwrap().message(),
        "portal response withheld"
    );
}

#[tokio::test]
async fn zone_null_name_and_quota_number_rendering_match_baseline() {
    let server = Server::start(vec![
        response("200 OK", br#"[{"name":null}]"#),
        response("200 OK", br#"[{"name":"cpu","used":1e2}]"#),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let zones = client
        .list_zones(&ctx, ListZonesInput::default())
        .await
        .unwrap();
    assert_eq!(zones.items[0].name, "");
    let quotas = client
        .list_quota_used(&ctx, ListQuotaUsedInput::default())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&quotas.items[0]).unwrap(),
        r#"{"name":"cpu","used":100}"#
    );
}

#[tokio::test]
async fn quota_lookup_does_not_match_quoted_string_representation() {
    let server = Server::start(vec![response("200 OK", br#"[{"name":"cpu"}]"#)]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .get_quota(
            &ctx,
            GetQuotaInput {
                name: "\"cpu\"".into(),
            },
        )
        .await;
    assert!(result.unwrap_err().error().has_class(ErrorClass::NotFound));
}

#[tokio::test]
async fn invalid_project_list_has_portal_invalid_response_error() {
    let server = Server::start(vec![response("200 OK", br#"{"zones":null}"#)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .list_zones(&ctx, ListZonesInput::default())
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.code(), "InvalidResponse");
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.message(), "invalid portal response; body withheld");
}

#[tokio::test]
async fn project_reads_use_vserver_and_preserve_output() {
    let server = Server::start(vec![
        response("200 OK", br#"{"zones":[{"id":"z1","name":"zone"}]}"#),
        response("200 OK", br#"{"quotas":[{"name":"cpu","used":2}]}"#),
        response("200 OK", br#"{"quota":{"limit":10}}"#),
    ]);
    let client = client(&server);
    let (ctx, _) = RequestContext::new();
    let zones = client
        .list_zones(&ctx, ListZonesInput::default())
        .await
        .unwrap();
    assert_eq!(zones.items[0].name, "zone");
    let quotas = client
        .list_quota_used(&ctx, ListQuotaUsedInput::default())
        .await
        .unwrap();
    assert_eq!(quotas.items[0].fields["used"], 2);
    let tag = client
        .get_tag_quota(&ctx, GetTagQuotaInput::default())
        .await
        .unwrap();
    assert_eq!(tag.tag_quota.fields["limit"], 10);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /v1/project-1/zones "));
    assert!(heads[1].starts_with(b"GET /v2/project-1/quotas/quotaUsed "));
    assert!(heads[2].starts_with(b"GET /v2/project-1/tag/quota "));
}

#[tokio::test]
async fn quota_lookup_matches_names_case_insensitively() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"[{"resourceName":"cpu","used":2}]"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_quota(&ctx, GetQuotaInput { name: "CPU".into() })
        .await
        .unwrap();
    assert_eq!(output.quota.fields["used"], 2);
}

#[test]
fn quota_lookup_runs_on_a_small_stack() {
    let server = Server::start(vec![response(
        "200 OK",
        br#"[{"resourceName":"cpu","used":2}]"#,
    )]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            runtime.block_on(async {
                let output = client(&server)
                    .get_quota(
                        &RequestContext::new().0,
                        GetQuotaInput { name: "CPU".into() },
                    )
                    .await
                    .unwrap();
                assert_eq!(output.quota.fields["used"], 2);
                assert_eq!(server.requests(), 1);
            });
        })
        .unwrap()
        .join()
        .unwrap();
}

fn response(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![
            ("Content-Length".into(), body.len().to_string()),
            ("Retry-After".into(), "1".into()),
        ],
        body: body.to_vec(),
    }
}

fn client(server: &Server) -> Client {
    client_with_bearer(server, BEARER)
}

fn client_with_bearer(server: &Server, bearer: &str) -> Client {
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_project_id("project-1".into())
        .with_static_token(Secret::new(bearer))
        .with_endpoint_overrides(EndpointOverrides {
            portal: Some(server.origin.clone()),
            vserver: Some(server.origin.clone()),
            ..Default::default()
        });
    let config = Config::new(options).unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    Client::new(&config)
}

#[tokio::test]
async fn user_info_uses_portal_get_and_keeps_api_keys() {
    let server = Server::start(vec![response("200 OK", USER)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap();
    assert_eq!(server.requests(), 1);
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /v1/users/info "));
    assert!(String::from_utf8_lossy(&heads[0]).contains(&format!("Bearer {BEARER}")));
    assert_eq!(output.user_info.fields["userId"], 100);
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["UserInfo"]["type"], "iam-user");
}

#[tokio::test]
async fn user_info_accepts_bare_objects_and_preserves_nested_api_keys() {
    let body = br#"{"extra":{"$serde_json::private::Number":"123","count":9007199254740993},"cash":123.4567890123456789}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap();
    let json = serde_json::to_string(&output).unwrap();
    assert_eq!(
        json,
        r#"{"UserInfo":{"cash":123.4567890123456789,"extra":{"$serde_json::private::Number":"123","count":9007199254740993}}}"#
    );
}

#[tokio::test]
async fn user_info_rejects_conflicting_or_malformed_objects() {
    let (ctx, _) = RequestContext::new();
    for body in [
        &b"[]"[..],
        b"1",
        br#""text""#,
        b"null",
        br#"{"data":null}"#,
        br#"{"data":[]}"#,
        br#"{"data":{},"userInfo":{}}"#,
        br#"{"DATA":{}}"#,
        br#"{"data":{},"DATA":{}}"#,
        br#"{"data":{"nested":{"foo":0,"foo":1}}}"#,
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let failure = client(&server)
            .get_user_info(&ctx, GetUserInfoInput::default())
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.code(), "InvalidResponse");
        assert_eq!(api.status(), Some(200));
    }
}

#[tokio::test]
async fn user_info_rejects_decoded_credential_reflection() {
    let body = format!(
        r#"{{"user":{{"unknown": "synthetic{}u002dportal-bearer"}}}}"#,
        char::from(92)
    );
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap_err();
    assert!(!format!("{failure:?}").contains(BEARER));
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.code(), "InvalidResponse");
    assert!(!api.message().contains(BEARER));
}

#[tokio::test]
async fn user_info_keeps_error_status_and_withholds_peer_text() {
    let body = format!(r#"{{"code":"PrivateCode","message":"{BEARER}"}}"#);
    let (ctx, _) = RequestContext::new();
    for (status, number, class, code) in [
        ("401 Unauthorized", 401, ErrorClass::Auth, "Unauthorized"),
        (
            "403 Forbidden",
            403,
            ErrorClass::PermissionDenied,
            "Forbidden",
        ),
        ("404 Not Found", 404, ErrorClass::NotFound, "NotFound"),
        (
            "429 Too Many Requests",
            429,
            ErrorClass::RateLimited,
            "Throttled",
        ),
    ] {
        let attempts = if number == 429 { 4 } else { 1 };
        let server = Server::start(
            (0..attempts)
                .map(|_| response(status, body.as_bytes()))
                .collect(),
        );
        let failure = client(&server)
            .get_user_info(&ctx, GetUserInfoInput::default())
            .await
            .unwrap_err();
        assert!(failure.error().has_class(class));
        assert!(!format!("{failure:?}").contains(BEARER));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(number));
        assert_eq!(api.code(), code);
        assert_eq!(api.message(), "portal response withheld");
        assert_eq!(api.retryable(), number == 429);
        assert_eq!(server.requests(), attempts);
    }
}

#[tokio::test]
async fn user_info_refuses_same_origin_redirect() {
    let server = Server::start(vec![
        Response::Http {
            status: "302 Found".into(),
            headers: vec![
                ("Location".into(), "/redirected".into()),
                ("Content-Length".into(), "0".into()),
            ],
            body: Vec::new(),
        },
        response("200 OK", USER),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await;
    assert!(result.is_err());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn user_info_rejects_oversized_body_without_partial_output() {
    let server = Server::start(vec![response("200 OK", &vec![b' '; (8 << 20) + 1])]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert!(failure.error().is_body_too_large());
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(api.message(), "portal response withheld");
}

#[tokio::test]
async fn user_info_preserves_number_spelling_without_creating_credential_text() {
    const NUMERIC_BEARER: &str = "+02";
    let body = br#"{"data":{"count":1E02,"zero":-0}}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (ctx, _) = RequestContext::new();
    let output = client_with_bearer(&server, NUMERIC_BEARER)
        .get_user_info(&ctx, GetUserInfoInput::default())
        .await
        .unwrap();
    let json = serde_json::to_string(&output).unwrap();
    assert!(!json.contains(NUMERIC_BEARER));
    assert_eq!(json, r#"{"UserInfo":{"count":1E02,"zero":-0}}"#);
}
