use super::*;

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
    let body = r#"[{"key":"env","value":"toy-secret","systemTag":false,"createdAt":""}]"#;
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
