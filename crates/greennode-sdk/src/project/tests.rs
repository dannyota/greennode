use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, CredentialsProvider, EndpointOverrides, Secret, Token};

const FIXTURE: &str = include_str!("../../tests/fixtures/project/list_projects.json");
const BEARER: &str = "synthetic-sdk-project-bearer";
const REFRESHED: &str = "synthetic-project-refreshed-bearer";

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
        ConfigOptions::default().with_static_token(Secret::new(BEARER)),
    )
}

fn client_with(server: &Server, options: ConfigOptions) -> Client {
    let config = Config::new(
        options
            .with_region("hcm-3".into())
            .with_project_id("configured-project".into())
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(server.origin.clone()),
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
async fn configured_project_does_not_replace_or_cache_explicit_inventory() {
    let inventory = r#"{"projects":[{"projectId":"north","region":"han-1"},{"projectId":"another"},{"projectId":"south","region":"hcm-3"}]}"#;
    let server = Server::start(vec![
        response("200 OK", FIXTURE),
        response("200 OK", inventory),
    ]);
    let client = client(&server);
    assert_eq!(server.requests(), 0);

    let (ctx, _) = RequestContext::new();
    let result = client
        .list_projects(&ctx, ListProjectsInput::default())
        .await
        .unwrap();
    assert_eq!(
        result.items,
        [Project {
            id: "project-1".into(),
            region: "hcm-3".into(),
            user_id: 100,
            ..Default::default()
        }]
    );

    let result = client
        .clone()
        .list_projects(
            &ctx,
            ListProjectsInput {
                region: "han-1".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|project| (project.id.as_str(), project.region.as_str()))
            .collect::<Vec<_>>(),
        [("north", "han-1"), ("another", "han-1")]
    );
    assert_eq!(server.requests(), 2);
    for head in server.heads() {
        assert!(head.starts_with(b"GET /v1/projects "));
        assert!(String::from_utf8_lossy(&head).contains(&format!("Bearer {BEARER}")));
    }
}

#[tokio::test]
async fn no_matching_project_returns_empty_items() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"projectId":"north","region":"han-1"}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_projects(&ctx, ListProjectsInput::default())
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&output).unwrap(), r#"{"Items":[]}"#);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn project_fields_decode_with_baseline_case_folding_and_null_defaults() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"projects":[{"ProjectID":"project-2","NAME":"prod","Region":null,"STATUS":null,"IsDefault":true,"UserID":null}]}"#,
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_projects(&ctx, ListProjectsInput::default())
        .await
        .unwrap();
    assert_eq!(
        output.items,
        [Project {
            id: "project-2".into(),
            name: "prod".into(),
            region: "hcm-3".into(),
            is_default: true,
            ..Default::default()
        }]
    );
}

#[tokio::test]
async fn invalid_inventories_fail_before_filtering() {
    let reflected = BEARER.replacen('s', r#"\u0073"#, 1);
    let cases = [
        "{}",
        r#"{"projects":null}"#,
        r#"{"projects":[],"nextPage":null}"#,
        r#"{"projects":[{"projectId":"","region":"han-1"}]}"#,
        r#"{"projects":[{"projectId":"duplicate"},{"projectId":"duplicate","region":"han-1"}]}"#,
        r#"{"projects":[{"projectId":"one","ProjectID":"two"}]}"#,
        &format!(r#"{{"projects":[{{"projectId":"one","region":"han-1","name":"{reflected}"}}]}}"#),
    ];
    let server = Server::start(cases.iter().map(|body| response("200 OK", body)).collect());
    let (ctx, _) = RequestContext::new();
    let client = client(&server);

    for _ in cases {
        let Err(failure) = client
            .list_projects(&ctx, ListProjectsInput::default())
            .await
        else {
            panic!("invalid or credential-bearing inventory was accepted");
        };
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotAWrite);
        assert!(!format!("{failure:?} {}", failure.error()).contains(BEARER));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(200));
        assert_eq!(api.operation(), "project.ListProjects");
        assert!(api.code() == "InvalidResponse");
    }
}

#[tokio::test]
async fn error_statuses_keep_identity_without_exposing_the_body() {
    let cases = [
        ("400 Bad Request", 400, ErrorClass::Api),
        ("401 Unauthorized", 401, ErrorClass::Auth),
        ("403 Forbidden", 403, ErrorClass::PermissionDenied),
        ("404 Not Found", 404, ErrorClass::NotFound),
    ];
    let body = format!(r#"{{"message":"untrusted body {BEARER}"}}"#);
    let server = Server::start(
        cases
            .iter()
            .map(|(status, ..)| response(status, &body))
            .collect(),
    );
    let (ctx, _) = RequestContext::new();
    let client = client(&server);

    for (_, status, class) in cases {
        let Err(failure) = client
            .list_projects(&ctx, ListProjectsInput::default())
            .await
        else {
            panic!("unexpected project listing success");
        };
        assert!(failure.error().has_class(class));
        let api = failure.error().api_error().unwrap();
        assert_eq!(api.status(), Some(status));
        assert_eq!(api.operation(), "project.ListProjects");
        let rendered = format!("{failure:?} {}", failure.error());
        assert!(!rendered.contains(BEARER));
        assert!(!rendered.contains("untrusted body"));
    }
    assert_eq!(server.requests(), cases.len());
}

#[tokio::test]
async fn rejects_even_a_same_origin_redirect() {
    let server = Server::start(vec![
        Response::Http {
            status: "302 Found".into(),
            headers: vec![
                ("Location".into(), "/redirect".into()),
                ("Content-Length".into(), "0".into()),
            ],
            body: vec![],
        },
        response("200 OK", FIXTURE),
    ]);
    let (ctx, _) = RequestContext::new();
    assert!(
        client(&server)
            .list_projects(&ctx, ListProjectsInput::default())
            .await
            .is_err()
    );
    assert_eq!(server.requests(), 1);
}

#[derive(Default)]
struct RotatingProvider(AtomicBool);

impl CredentialsProvider for RotatingProvider {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async {
            let value = if self.0.load(Ordering::Relaxed) {
                REFRESHED
            } else {
                BEARER
            };
            Ok(Token::new(Secret::new(value), None))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        rejected: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async {
            if rejected.expose_secret() == BEARER {
                self.0.store(true, Ordering::Relaxed);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn non_json_unauthorized_refreshes_and_reflection_of_previous_bearer_is_refused() {
    let echo = format!(r#"{{"projects":[{{"projectId":"{BEARER}"}}]}}"#);
    let server = Server::start(vec![
        response("401 Unauthorized", "unauthorized"),
        response("200 OK", &echo),
    ]);
    let client = client_with(
        &server,
        ConfigOptions::default().with_credentials_provider(Arc::new(RotatingProvider::default())),
    );
    let (ctx, _) = RequestContext::new();
    let Err(failure) = client
        .list_projects(&ctx, ListProjectsInput::default())
        .await
    else {
        panic!("reflected earlier credential was accepted");
    };
    let rendered = format!("{failure:?} {}", failure.error());
    assert!(!rendered.contains(BEARER));
    assert!(!rendered.contains(REFRESHED));
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.status(), Some(200));
    assert!(api.code() == "InvalidResponse");

    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    for (head, token) in heads.iter().zip([BEARER, REFRESHED]) {
        assert!(String::from_utf8_lossy(head).contains(&format!("Bearer {token}")));
    }
}

#[tokio::test]
async fn non_json_gateway_error_is_retried() {
    let body = b"<html>unavailable</html>";
    let server = Server::start(vec![
        Response::Http {
            status: "503 Service Unavailable".into(),
            headers: vec![
                ("Retry-After".into(), "1".into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            body: body.to_vec(),
        },
        response("200 OK", FIXTURE),
    ]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_projects(&ctx, ListProjectsInput::default())
        .await
        .unwrap();
    assert_eq!(output.items.len(), 1);
    assert_eq!(server.requests(), 2);
}
