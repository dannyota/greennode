use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{ConfigOptions, EndpointOverrides, Secret};

fn test_client(server: &Server, token: &str) -> Client {
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new(token))
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

fn http_response(body: &str) -> Response {
    Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn null_and_missing_created_at_decode_as_empty() {
    let body = br#"[{"key":"a","value":"1","systemTag":false,"createdAt":null},{"key":"b","value":"2","systemTag":true}]"#;
    let tags = decode_snapshot(body).unwrap();
    assert!(tags.iter().all(|tag| tag.created_at.is_empty()));
}

#[tokio::test]
async fn reflected_boolean_is_withheld() {
    let server = Server::start(vec![http_response(
        r#"[{"key":"a","value":"b","systemTag":true}]"#,
    )]);
    let (ctx, _) = RequestContext::new();
    assert!(
        test_client(&server, "true")
            .list_resource_tags(&ctx, ListResourceTagsInput::new("res-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn malformed_json_reports_decode_diagnostic() {
    let server = Server::start(vec![http_response("[")]);
    let (ctx, _) = RequestContext::new();
    let failure = test_client(&server, "toy-token")
        .list_resource_tags(&ctx, ListResourceTagsInput::new("res-1"))
        .await
        .unwrap_err();
    let api = failure.error().api_error().unwrap();
    assert_eq!(api.operation(), "tagging.ListResourceTags");
    assert_eq!(api.message(), "response failed to decode");
}

#[test]
fn snapshot_requires_complete_unique_keys() {
    assert!(decode_snapshot(br#"[{"key":"team","value":"blue","systemTag":false}]"#).is_ok());
    assert!(decode_snapshot(br#"[{"key":"team","value":"blue"}]"#).is_err());
    assert!(decode_snapshot(br#"[{"key":"team","value":"blue","systemTag":false},{"key":"team","value":"red","systemTag":false}]"#).is_err());
    assert!(decode_snapshot(b"null").is_err());
}

#[tokio::test]
async fn list_reads_the_resource_and_redacts_reflected_credentials() {
    use crate::wire::test_server::{Response, Server};
    use crate::{ConfigOptions, EndpointOverrides, Secret};
    let body = r#"[{"key":"env","value":"toy%2Dsecret","systemTag":false,"createdAt":""}]"#;
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new("toy-secret"))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let output = Client::new(&config)
        .list_resource_tags(&ctx, ListResourceTagsInput::new("res-1"))
        .await
        .unwrap();
    assert_eq!(output.items[0].value, "<redacted>");
    assert!(server.heads()[0].starts_with(b"GET /v2/project-1/tag/resource/res-1 "));
}

#[tokio::test]
async fn tag_replaces_once_and_confirms() {
    let before = r#"[{"key":"env","value":"old","systemTag":false},{"key":"vng.zone","value":"zone","systemTag":true}]"#;
    let after = r#"[{"key":"env","value":"new","systemTag":false},{"key":"vng.zone","value":"zone","systemTag":true}]"#;
    let server = Server::start(vec![
        http_response(before),
        http_response(""),
        http_response(after),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = test_client(&server, "toy-token")
        .tag_resource(
            &ctx,
            TagResourceInput {
                resource_id: "res-1".into(),
                resource_type: "SERVER".into(),
                key: "env".into(),
                value: "new".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result["Previous"], "old");
    assert_eq!(result["Changed"], true);
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[0].starts_with(b"GET "));
    assert!(heads[1].starts_with(b"PUT "));
    assert!(heads[2].starts_with(b"GET "));
}

#[tokio::test]
async fn untag_noop_sends_no_write() {
    let server = Server::start(vec![http_response("[]")]);
    let (ctx, _) = RequestContext::new();
    let result = test_client(&server, "toy-token")
        .untag_resource(
            &ctx,
            UntagResourceInput {
                resource_id: "res-1".into(),
                resource_type: "SERVER".into(),
                key: "absent".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result["Changed"], false);
    assert_eq!(server.heads().len(), 1);
}

struct Rotating(std::sync::atomic::AtomicUsize);
impl crate::CredentialsProvider for Rotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::Token, Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let count = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::Token::new(
                Secret::new(if count == 0 {
                    "old-toy-bearer"
                } else {
                    "new-toy-bearer"
                }),
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
async fn discovery_bearer_stays_redacted_after_rotation() {
    let before = r#"[{"key":"env","value":"old-toy-bearer","systemTag":false}]"#;
    let after = r#"[{"key":"env","value":"new","systemTag":false},{"key":"other","value":"old-toy-bearer","systemTag":true}]"#;
    let server = Server::start(vec![
        http_response(r#"{"projects":[{"projectId":"project-1"}]}"#),
        http_response(before),
        http_response(""),
        http_response(after),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_credentials_provider(Arc::new(Rotating(std::sync::atomic::AtomicUsize::new(0))))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let output = Client::new(&config)
        .tag_resource(&ctx, write_input("env"))
        .await
        .unwrap();
    assert_eq!(output["Previous"], "<redacted>");
    assert_eq!(output["Tags"][1]["Value"], "<redacted>");
    assert!(!output.to_string().contains("old-toy-bearer"));
}

fn write_input(key: &str) -> TagResourceInput {
    TagResourceInput {
        resource_id: "res-1".into(),
        resource_type: "SERVER".into(),
        key: key.into(),
        value: "new".into(),
    }
}

fn error_response(status: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), "0".into())],
        body: Vec::new(),
    }
}

#[tokio::test]
async fn failed_write_and_confirm_are_not_settled_with_partial() {
    let before = r#"[{"key":"env","value":"toy-token","systemTag":false}]"#;
    for responses in [
        vec![
            http_response(before),
            error_response("500 Internal Server Error"),
        ],
        vec![http_response(before), http_response(""), http_response("[")],
    ] {
        let server = Server::start(responses);
        let (ctx, _) = RequestContext::new();
        let failure = test_client(&server, "toy-token")
            .tag_resource(&ctx, write_input("env"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotSettled);
        assert!(failure.error().has_class(ErrorClass::Api));
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
        assert_eq!(failure.partial().unwrap()["Previous"], "<redacted>");
    }
}

#[tokio::test]
async fn rejected_write_has_no_partial() {
    let server = Server::start(vec![http_response("[]"), error_response("400 Bad Request")]);
    let (ctx, _) = RequestContext::new();
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("env"))
        .await
        .unwrap_err();
    assert!(failure.partial().is_none());
    assert!(!failure.error().has_class(ErrorClass::NotSettled));
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn system_tag_refusal_has_baseline_identity() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("vng.zone"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::SystemTag)
    );
    assert!(failure.error().api_error().is_none());
    assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    assert!(server.heads().is_empty());
}

fn refusal_text(failure: &Failure<serde_json::Value>) -> String {
    failure
        .error()
        .detail::<SystemTagText>()
        .map(|text| text.message().to_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn system_tag_refusal_names_operation_resource_and_quoted_key() {
    let system = r#"[{"key":"zone","value":"a","systemTag":true}]"#;
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let client = test_client(&server, "toy-token");
    let failure = client
        .tag_resource(&ctx, write_input("vng.a\"b\u{85}"))
        .await
        .unwrap_err();
    assert_eq!(
        refusal_text(&failure),
        "tagging: key is a system tag: tagging.TagResource: key \"vng.a\\\"b\\u0085\""
    );
    let failure = client
        .untag_resource(
            &ctx,
            UntagResourceInput {
                resource_id: "res-1".into(),
                resource_type: "SERVER".into(),
                key: "vng.zone".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        refusal_text(&failure),
        "tagging: key is a system tag: tagging.UntagResource: key \"vng.zone\""
    );
    let server = Server::start(vec![http_response(system)]);
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("zone"))
        .await
        .unwrap_err();
    assert_eq!(
        refusal_text(&failure),
        "tagging: key is a system tag: tagging.TagResource: resource res-1: key \"zone\""
    );
}

#[tokio::test]
async fn system_tag_refusal_falls_back_to_fixed_text_for_a_reflected_credential() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("vng.toy-token"))
        .await
        .unwrap_err();
    assert_eq!(refusal_text(&failure), "tagging: key is a system tag");
    let server = Server::start(vec![http_response(
        r#"[{"key":"toy-token","value":"a","systemTag":true}]"#,
    )]);
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("toy-token"))
        .await
        .unwrap_err();
    assert_eq!(refusal_text(&failure), "tagging: key is a system tag");
}

#[tokio::test]
async fn system_tag_refusal_keeps_a_long_key_whole() {
    let key = format!("vng.{}", "k".repeat(900));
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input(&key))
        .await
        .unwrap_err();
    assert_eq!(
        refusal_text(&failure),
        format!("tagging: key is a system tag: tagging.TagResource: key \"{key}\"")
    );
}

#[tokio::test]
async fn system_tag_prefix_screen_survives_a_cancelled_context() {
    let (ctx, handle) = RequestContext::new();
    handle.cancel();
    let server = Server::start(vec![]);
    let failure = test_client(&server, "toy-token")
        .tag_resource(&ctx, write_input("vng.toy-token"))
        .await
        .unwrap_err();
    assert_eq!(refusal_text(&failure), "tagging: key is a system tag");
}

#[tokio::test]
async fn tag_read_exit_refuses_a_reflected_redaction_marker() {
    let raw = br#"[{"key":"env","value":"redacted","systemTag":false}]"#;
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.to_vec(),
    }]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_project_id("project-1".into())
            .with_static_token(Secret::new("redacted"))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let result = Client::new(&config)
        .list_resource_tags(
            &RequestContext::new().0,
            ListResourceTagsInput::new("res-1"),
        )
        .await;
    assert_eq!(server.requests(), 1);
    assert!(result.is_err());
}
