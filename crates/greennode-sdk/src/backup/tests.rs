use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{
    ConfigOptions, CredentialsProvider, EndpointOverrides, ErrorClass, Secret, Token, WriteDispatch,
};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};

const BACKENDS: &str = include_str!("../../tests/fixtures/backup/backends.json");
const POLICIES: &str = include_str!("../../tests/fixtures/backup/policies.json");
const POLICY: &str = include_str!("../../tests/fixtures/backup/get-policy.json");
const EMPTY: &str = include_str!("../../tests/fixtures/backup/empty.json");
const TOKEN: &str = "synthetic-backup-bearer";

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server, region: &str) -> Client {
    configured_client(
        server,
        region,
        ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
    )
}

fn configured_client(server: &Server, region: &str, options: ConfigOptions) -> Client {
    let config = Config::new(
        options
            .with_region(region.into())
            .with_project_id("unscoped-project".into())
            .with_endpoint_overrides(EndpointOverrides {
                backup_center: Some(format!("{}/vbackup-gateway/", server.origin)),
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
async fn reads_use_regional_backup_center_without_project_scope() {
    for region in ["hcm-3", "han-1"] {
        let server = Server::start(vec![
            response("200 OK", BACKENDS),
            response("200 OK", POLICIES),
            response("200 OK", EMPTY),
            response("200 OK", POLICY),
        ]);
        let client = client(&server, region);
        let (ctx, _) = RequestContext::new();
        let backends = client.list_backends(&ctx, ListBackendsInput).await.unwrap();
        assert_eq!(backends.items[0].id, "backend-1");
        assert_eq!((backends.page, backends.page_size), (None, None));
        assert_eq!((backends.total_page, backends.total_item), (1, 1));
        let policies = client
            .list_policies(&ctx, ListPoliciesInput::default())
            .await
            .unwrap();
        assert_eq!(
            (
                policies.page,
                policies.page_size,
                policies.total_page,
                policies.total_item
            ),
            (1, 200, 2, 3)
        );
        assert_eq!(policies.items[0].config.hour, 1);
        assert_eq!(policies.items[0].config.minute, 30);
        assert_eq!(
            policies.items[0]
                .config
                .daily_config
                .as_ref()
                .unwrap()
                .retention,
            Some(7)
        );
        let empty = client
            .list_policies(&ctx, ListPoliciesInput { page: 2, size: 10 })
            .await
            .unwrap();
        assert!(empty.items.is_empty());
        let detail = client
            .clone()
            .get_policy(&ctx, GetPolicyInput::new("synthetic"))
            .await
            .unwrap();
        assert_eq!(detail.policy.id, "synthetic");
        assert_eq!(
            detail
                .policy
                .config
                .daily_config
                .unwrap()
                .incremental_quantity,
            Some(0)
        );
        let heads = server.heads();
        assert_eq!(heads.len(), 4);
        for (head, target) in heads.iter().zip([
            "backends",
            "backup-policies?page=1&size=200",
            "backup-policies?page=2&size=10",
            "backup-policies/synthetic",
        ]) {
            assert!(head.starts_with(format!("GET /vbackup-gateway/v1/{target} ").as_bytes()));
            let head = String::from_utf8_lossy(head).to_ascii_lowercase();
            assert!(!head.contains("projectid:"));
            assert!(head.contains(&format!("authorization: bearer {TOKEN}")));
        }
    }
}

#[tokio::test]
async fn retained_response_strings_refuse_reflected_bearer() {
    let reflected = format!("prefix-{}-suffix", TOKEN.replace('-', "\\u002d"));
    let backend = BACKENDS.replace("\"backend\"", &format!("\"{reflected}\""));
    let policies = POLICIES.replace("\"incremental\"", &format!("\"{reflected}\""));
    let policy = POLICY.replace(
        "\"timeZone\": \"synthetic\"",
        &format!("\"timeZone\": \"{reflected}\""),
    );
    let server = Server::start(vec![
        response("200 OK", &backend),
        response("200 OK", &policies),
        response("200 OK", &policy),
        response("200 OK", &BACKENDS.replace("SECRET-MARKER", TOKEN)),
    ]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let backends = client
        .list_backends(&ctx, ListBackendsInput)
        .await
        .unwrap_err();
    let policies = client
        .list_policies(&ctx, ListPoliciesInput::default())
        .await
        .unwrap_err();
    let policy = client
        .get_policy(&ctx, GetPolicyInput::new("synthetic"))
        .await
        .unwrap_err();
    assert!(
        backends.partial().is_none() && policies.partial().is_none() && policy.partial().is_none()
    );
    for error in [backends.error(), policies.error(), policy.error()] {
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(200));
        assert_eq!(api.code(), "InvalidResponse");
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    }
    let omitted = client.list_backends(&ctx, ListBackendsInput).await.unwrap();
    assert!(!serde_json::to_string(&omitted).unwrap().contains(TOKEN));
}

#[tokio::test]
async fn retained_response_numbers_refuse_reflected_bearer() {
    let token = "9007199254740992";
    let backend = BACKENDS.replace("\"totalItems\": 1", &format!("\"totalItems\": {token}"));
    let policies = POLICIES.replace(
        "\"backupInstanceCount\": 3",
        "\"backupInstanceCount\": 9007199254740993",
    );
    let policy = POLICY.replace("\"retention\": 0", "\"retention\": 90071992547409920e-1");
    let server = Server::start(vec![
        response("200 OK", &backend),
        response("200 OK", &policies),
        response("200 OK", &policy),
        response("200 OK", BACKENDS),
    ]);
    let client = configured_client(
        &server,
        "han-1",
        ConfigOptions::default().with_static_token(Secret::new(token)),
    );
    let (ctx, _) = RequestContext::new();
    let Err(backends) = client.list_backends(&ctx, ListBackendsInput).await else {
        panic!("reflected totalItems accepted");
    };
    let Err(policies) = client
        .list_policies(&ctx, ListPoliciesInput::default())
        .await
    else {
        panic!("reflected backupInstanceCount accepted");
    };
    let Err(policy) = client
        .get_policy(&ctx, GetPolicyInput::new("synthetic"))
        .await
    else {
        panic!("reflected retention accepted");
    };
    assert!(
        backends.partial().is_none() && policies.partial().is_none() && policy.partial().is_none()
    );
    for error in [backends.error(), policies.error(), policy.error()] {
        assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
        assert!(!format!("{error} {error:?}").contains(token));
    }
    assert!(client.list_backends(&ctx, ListBackendsInput).await.is_ok());
}

#[tokio::test]
async fn validates_region_paths_and_paging_before_sending() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let unsupported = client(&server, "hcm-4");
    let failure = unsupported
        .list_backends(&ctx, ListBackendsInput)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    let failure = unsupported
        .list_policies(&ctx, ListPoliciesInput::default())
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    let failure = unsupported
        .get_policy(&ctx, GetPolicyInput::new("policy-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::InvalidConfig);
    let supported = client(&server, "han-1");
    for input in [
        ListPoliciesInput { page: -1, size: 0 },
        ListPoliciesInput { page: 0, size: -1 },
    ] {
        let failure = supported.list_policies(&ctx, input).await.unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
    }
    for id in ["", "../private-policy"] {
        let failure = supported
            .get_policy(&ctx, GetPolicyInput::new(id))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert!(!failure.error().to_string().contains("private-policy"));
    }
    assert_eq!(server.requests(), 0);
}

const FIRST: &str = "synthetic-backup-first";
const SECOND: &str = "synthetic-backup-second";

#[derive(Default)]
struct RotatingProvider(AtomicUsize);

impl CredentialsProvider for RotatingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        let value = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            FIRST
        } else {
            SECOND
        };
        Box::pin(async move { Ok(Token::new(Secret::new(value), None)) })
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
async fn lists_refuse_credentials_from_either_attempt_after_401() {
    let responses = [
        ("id", "backend-1", BACKENDS, false),
        ("timeZone", "UTC", POLICIES, true),
    ];
    for (field, value, fixture, policies) in responses {
        for credential in [FIRST, SECOND] {
            let raw = fixture.replace(
                &format!("\"{field}\": \"{value}\""),
                &format!(
                    "\"{field}\": \"prefix-{}-suffix\"",
                    credential.replace('-', "\\u002d")
                ),
            );
            let server = Server::start(vec![
                response("401 Unauthorized", "{}"),
                response("200 OK", &raw),
            ]);
            let provider = Arc::new(RotatingProvider::default());
            let client = configured_client(
                &server,
                "han-1",
                ConfigOptions::default().with_credentials_provider(provider),
            );
            let (ctx, _) = RequestContext::new();
            let error = if policies {
                client
                    .list_policies(&ctx, ListPoliciesInput::default())
                    .await
                    .unwrap_err()
                    .into_parts()
                    .0
            } else {
                client
                    .list_backends(&ctx, ListBackendsInput)
                    .await
                    .unwrap_err()
                    .into_parts()
                    .0
            };
            let api = error.api_error().unwrap();
            assert_eq!(api.status(), Some(200));
            assert_eq!(api.code(), "InvalidResponse");
            let text = format!("{error} {error:?}");
            assert!(!text.contains(FIRST) && !text.contains(SECOND));
            let heads = server.heads();
            assert_eq!(heads.len(), 2);
            for (head, token) in heads.iter().zip([FIRST, SECOND]) {
                assert!(String::from_utf8_lossy(head).contains(&format!("Bearer {token}")));
            }
        }
    }
}

#[tokio::test]
async fn malformed_shapes_never_yield_partial_results() {
    let invalid_lists = [
        r#"{"items":null,"page":null,"pageSize":null,"totalPages":0,"totalItems":0}"#,
        r#"{"items":[],"pageSize":null,"totalPages":0,"totalItems":0}"#,
        r#"{"items":[],"page":null,"pageSize":null,"totalPages":null,"totalItems":0}"#,
    ];
    let invalid_details = [
        r#"{"Id":"policy-1","config":{}}"#,
        r#"{"id":"policy-1","config":null}"#,
        r#"{"id":"different-policy","config":{}}"#,
        r#"{"id":"policy-1","config":{"hour":1,"Hour":2}}"#,
        r#"{"id":"policy-1","config":{"dailyConfig":{"retention":1,"retention":2}}}"#,
        r#"{"id":"policy-1","config":{},"unknown":{"a":1,"a":2}}"#,
    ];
    let server = Server::start(
        invalid_lists
            .iter()
            .chain([BACKENDS].iter())
            .chain(invalid_details.iter())
            .map(|body| response("200 OK", body))
            .collect(),
    );
    let client = client(&server, "hcm-3");
    let (ctx, _) = RequestContext::new();
    for _ in invalid_lists {
        let failure = client
            .list_backends(&ctx, ListBackendsInput)
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(200));
        assert_eq!(
            api.message(),
            "unrecognized Backup Center response; body withheld"
        );
    }
    let failure = client
        .list_policies(&ctx, ListPoliciesInput::default())
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert_eq!(
        api.message(),
        "unrecognized Backup Center response; body withheld"
    );
    for _ in invalid_details {
        let failure = client
            .get_policy(&ctx, GetPolicyInput::new("policy-1"))
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(200));
        assert_eq!(
            api.message(),
            "unrecognized Backup Center response; body withheld"
        );
        assert!(!failure.error().to_string().contains("different-policy"));
    }
}

#[tokio::test]
async fn malformed_json_reports_decode_failure_for_each_read() {
    let body = format!(r#"{{"name":"{TOKEN}","#);
    let server = Server::start((0..3).map(|_| response("200 OK", &body)).collect());
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    let backends = client
        .list_backends(&ctx, ListBackendsInput)
        .await
        .unwrap_err();
    let policies = client
        .list_policies(&ctx, ListPoliciesInput::default())
        .await
        .unwrap_err();
    let policy = client
        .get_policy(&ctx, GetPolicyInput::new("policy-1"))
        .await
        .unwrap_err();
    for error in [backends.error(), policies.error(), policy.error()] {
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(200));
        assert_eq!(api.message(), "response failed to decode; body withheld");
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    }
}

#[tokio::test]
async fn schedule_preserves_whole_integers_and_nullable_daily_fields() {
    let body = |config: &str| format!(r#"{{"id":"policy-1","config":{config}}}"#);
    let configs = [
        r#"{}"#,
        r#"{"dailyConfig":null}"#,
        r#"{"dailyConfig":{}}"#,
        r#"{"hour":9223372036854775807.0,"minute":-92233720368547758080e-1,"dailyConfig":{"retention":0.0,"backupType":"","incrementalQuantity":12e1}}"#,
        r#"{"hour":1.5}"#,
        r#"{"minute":"7"}"#,
        r#"{"hour":9223372036854775808.0}"#,
        r#"{"dailyConfig":{"retention":1e-65}}"#,
    ];
    let server = Server::start(
        configs
            .iter()
            .map(|config| response("200 OK", &body(config)))
            .collect(),
    );
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    for expected in [None, None, Some(DailyConfig::default())] {
        let result = client
            .get_policy(&ctx, GetPolicyInput::new("policy-1"))
            .await
            .unwrap();
        assert_eq!(result.policy.config.daily_config, expected);
    }
    let result = client
        .get_policy(&ctx, GetPolicyInput::new("policy-1"))
        .await
        .unwrap();
    assert_eq!(
        (result.policy.config.hour, result.policy.config.minute),
        (i64::MAX, i64::MIN)
    );
    let daily = result.policy.config.daily_config.unwrap();
    assert_eq!(daily.retention, Some(0));
    assert_eq!(daily.backup_type.as_deref(), Some(""));
    assert_eq!(daily.incremental_quantity, Some(120));
    for _ in 4..configs.len() {
        let failure = client
            .get_policy(&ctx, GetPolicyInput::new("policy-1"))
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        assert_eq!(failure.error().api_error().unwrap().status(), Some(200));
    }
}

#[tokio::test]
async fn status_and_overflow_errors_withhold_peer_fields() {
    let body = format!(r#"{{"code":"{TOKEN}","message":"{TOKEN}"}}"#);
    let oversized = "x".repeat((8 << 20) + 1);
    let server = Server::start(vec![
        response("401 Unauthorized", &body),
        response("403 Forbidden", &body),
        response("404 Not Found", &body),
        response("500 Internal Server Error", &body),
        response("200 OK", &oversized),
    ]);
    let client = client(&server, "han-1");
    let (ctx, _) = RequestContext::new();
    for (status, class, code) in [
        (401, ErrorClass::Auth, "Unauthorized"),
        (403, ErrorClass::PermissionDenied, "Forbidden"),
        (404, ErrorClass::NotFound, "NotFound"),
        (500, ErrorClass::Api, "ServerError"),
        (200, ErrorClass::Api, ""),
    ] {
        let failure = client
            .get_policy(&ctx, GetPolicyInput::new("policy-1"))
            .await
            .unwrap_err();
        assert!(failure.partial().is_none());
        let error = failure.error();
        assert_eq!(error.class(), class);
        assert_eq!(error.is_body_too_large(), status == 200);
        let api = error.api_error().unwrap();
        assert_eq!(api.status(), Some(status));
        assert_eq!(api.code(), code);
        assert_eq!(api.operation(), "backup.GetPolicy");
        assert_eq!(api.message(), "Backup Center response withheld");
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
}
