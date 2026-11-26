#![cfg(feature = "test-support")]

#[path = "cdn/writes.rs"]
mod writes;

use greennode_sdk::cdn::*;
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{Config, ConfigOptions, EndpointOverrides, ErrorClass, RequestContext, Secret};

const CDN_KEY: &str = "toy-cdn-key";
const MANAGEMENT: &str = "toy-management-token";

fn response(body: &str, media: &str) -> Response {
    with_status("200 OK", body, media)
}

fn with_status(status: &str, body: &str, media: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![
            ("Content-Length".into(), body.len().to_string()),
            ("Content-Type".into(), media.into()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

fn client(server: &Server, key: Option<&str>) -> Client {
    client_with_docs_path(server, key, "/faq/vcdn")
}

fn client_with_docs_path(server: &Server, key: Option<&str>, docs_path: &str) -> Client {
    let mut options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(MANAGEMENT))
        .with_endpoint_overrides(EndpointOverrides {
            cdn_docs: Some(format!("{}{docs_path}", server.origin)),
            cdn: Some(format!("{}/vcdn-api/", server.origin)),
            ..Default::default()
        });
    if let Some(key) = key {
        options = options.with_cdn_api_key(Secret::new(key));
    }
    Client::new(&Config::new(with_product_loopback(Product::Cdn, options, server)).unwrap())
}

#[tokio::test]
async fn public_page_without_a_key_has_canonical_ranges() {
    let server = Server::start(vec![response(
        include_str!("fixtures/cdn/faq-vcdn.html"),
        "text/html; charset=utf-8",
    )]);
    let (ctx, _) = RequestContext::new();
    let ranges = client(&server, None)
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap();
    assert_eq!(ranges.items.len(), 19);
    assert_eq!(ranges.items[0], "14.225.2.32/28");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /faq/vcdn "));
    assert!(
        !String::from_utf8_lossy(&heads[0])
            .to_ascii_lowercase()
            .contains("authorization:")
    );
}

#[tokio::test]
async fn missing_key_refuses_api_reads_before_sending() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server, None)
        .list_web_accelerators(&ctx, ListWebAcceleratorsInput)
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NoCredentials);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn typed_reflection_is_redacted_in_output() {
    let server = Server::start(vec![response(
        r#"{"success":true,"data":[{"cdnId":42,"domainName":"142.test","status":42}]}"#,
        "application/json",
    )]);
    let (ctx, _) = RequestContext::new();
    let list = client(&server, Some("42"))
        .list_web_accelerators(&ctx, ListWebAcceleratorsInput)
        .await
        .unwrap();
    let output = serde_json::to_value(list).unwrap();
    for field in ["CDNID", "DomainName", "Status", "StatusName"] {
        assert_eq!(output["Items"][0][field], "<redacted>");
    }
}

#[tokio::test]
async fn analytics_posts_are_reads_and_preserve_the_report_shape() {
    let server = Server::start(vec![response(
        include_str!("fixtures/cdn/analytics-traffic-report.json"),
        "application/json",
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, Some(CDN_KEY))
        .get_traffic_report(
            &ctx,
            GetTrafficReportInput::new(vec!["cdn.example.test".into()], "2025-01-01", "2025-01-03"),
        )
        .await
        .unwrap();
    assert!(!output.items.is_empty());
    let heads = server.heads();
    assert!(heads[0].starts_with(b"POST /vcdn-api/v1/analytic/traffic-report "));
    let head = String::from_utf8_lossy(&heads[0]).to_ascii_lowercase();
    assert!(head.contains("authorization: bearer toy-cdn-key"));
    assert!(!head.contains(MANAGEMENT));
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"domains":["cdn.example.test"],"fromTime":"01/01/2025","toTime":"03/01/2025"})
    );
}

#[tokio::test]
async fn malformed_public_sections_fail_without_a_partial_list() {
    let cases = [
        include_str!("fixtures/cdn/bare-address.html"),
        include_str!("fixtures/cdn/heading-duplicate.html"),
        include_str!("fixtures/cdn/heading-missing.html"),
        include_str!("fixtures/cdn/heading-only-in-script.html"),
        include_str!("fixtures/cdn/host-bits-set.html"),
        include_str!("fixtures/cdn/invalid-length.html"),
        include_str!("fixtures/cdn/no-heading-after.html"),
        include_str!("fixtures/cdn/section-no-cidr.html"),
    ];
    let (ctx, _) = RequestContext::new();
    for body in cases {
        let server = Server::start(vec![response(body, "text/html")]);
        let error = client(&server, None)
            .list_ip_ranges(&ctx, ListIpRangesInput)
            .await
            .unwrap_err();
        assert_eq!(
            error.error().detail::<ErrorDetail>(),
            Some(&ErrorDetail::PageFormat)
        );
        assert!(error.partial().is_none());
    }
}

#[tokio::test]
async fn oversized_public_responses_distinguish_page_format_from_http_status() {
    let oversized = "x".repeat((4 << 20) + 1);
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![response(&oversized, "text/html")]);
    let error = client(&server, None)
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::PageFormat)
    );
    let server = Server::start(vec![with_status("404 Not Found", &oversized, "text/html")]);
    let error = client(&server, None)
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    assert_eq!(error.error().api_error().unwrap().status(), Some(404));
}

#[tokio::test]
async fn invalid_detail_cannot_be_mistaken_for_matching_state() {
    let (ctx, _) = RequestContext::new();
    for body in [
        r#"{"success":true,"data":{"cdnId":"cdn-1"}}"#,
        r#"{"success":true,"data":{"cdnId":"cdn-2","status":1}}"#,
        r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"Status":0}}"#,
    ] {
        let server = Server::start(vec![response(body, "application/json")]);
        let failure = client(&server, Some(CDN_KEY))
            .get_web_accelerator(&ctx, GetWebAcceleratorInput::new("cdn-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().api_error().unwrap().code(), "EmptyResponse");
    }
    let server = Server::start(vec![response(
        include_str!("fixtures/cdn/webaccelerator-not-found.json"),
        "application/json",
    )]);
    let failure = client(&server, Some(CDN_KEY))
        .get_web_accelerator(&ctx, GetWebAcceleratorInput::new("cdn-1"))
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::NotFound);
    assert_eq!(failure.error().api_error().unwrap().status(), Some(200));
}

#[tokio::test]
async fn raw_rule_values_cannot_echo_a_key_via_duplicate_members() {
    let body = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"childrenRule":{"rule":"toy-cdn-key","rule":"safe"}}}"#;
    let server = Server::start(vec![response(body, "application/json")]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, Some(CDN_KEY))
        .get_web_accelerator(&ctx, GetWebAcceleratorInput::new("cdn-1"))
        .await;
    if let Ok(output) = result {
        let text = serde_json::to_string(&output).unwrap();
        assert!(!text.contains(CDN_KEY));
    }
}

#[tokio::test]
async fn encoded_reflections_are_withheld_from_all_cdn_reads() {
    let field_echo = r#"{"success":true,"data":[{"domainName":"toy%2Dcdn%2Dkey.example.test"},{"domainName":"safe.example.test"}]}"#;
    let raw_echo = r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"childrenRule":{"rule":"toy%252Dcdn%252Dkey","safe":"kept"}}}"#;
    let counts_echo = r#"{"success":true,"data":"{\"TOY%2DCDN%2DKEY\":3,\"hit\":4}"}"#;
    let server = Server::start(vec![
        response(field_echo, "application/json"),
        response(raw_echo, "application/json"),
        response(counts_echo, "application/json"),
        response("<html></html>", "text/plain; echo=toy%2Dcdn%2Dkey"),
        response("<html></html>", r"text/plain; echo=toy\\u002dcdn\\u002dkey"),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, Some(CDN_KEY));
    let list = client
        .list_web_accelerators(&ctx, ListWebAcceleratorsInput)
        .await
        .unwrap();
    assert!(
        list.items[0].domain_name == "<redacted>",
        "encoded typed value survived"
    );
    assert_eq!(list.items[1].domain_name, "safe.example.test");
    let detail = client
        .get_web_accelerator(&ctx, GetWebAcceleratorInput::new("cdn-1"))
        .await
        .unwrap();
    let detail = serde_json::to_value(detail).unwrap();
    assert!(
        detail["WebAccelerator"]["PageRules"]["rule"] == "<redacted>",
        "encoded raw value survived"
    );
    assert_eq!(detail["WebAccelerator"]["PageRules"]["safe"], "kept");
    let output = client
        .get_cache_status(&ctx, analytics_range("cdn.example.test"))
        .await
        .unwrap();
    let output = serde_json::to_value(output).unwrap();
    assert_eq!(output["Counts"].as_object().unwrap().len(), 1);
    assert_eq!(output["Counts"]["hit"].as_f64(), Some(4.0));
    let error = client
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap_err();
    let message = error.error().api_error().unwrap().message();
    assert!(
        !message.contains("toy%2Dcdn%2Dkey"),
        "encoded diagnostic value survived"
    );
    let error = client
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap_err();
    let message = error.error().api_error().unwrap().message();
    assert!(
        !message.contains("echo="),
        "escaped diagnostic value survived"
    );
}

#[tokio::test]
async fn public_page_ignores_hidden_tags_and_comment_decoys_within_a_bound() {
    let decoys = "</script title='>>'><h3>CDN IP range</h3>203.0.113.0/24".repeat(16_384);
    let unterminated = "</script title='".repeat(16_384);
    let legitimate = "<h3>CDN IP range</h3><p>192.0.2.0/24</p><h3>Next</h3>";
    let cases = [
        format!("<script><!--{decoys}--></script>{legitimate}"),
        format!("{legitimate}<script>{unterminated}"),
    ];
    let (ctx, _) = RequestContext::new();
    for body in cases {
        let server = Server::start(vec![response(&body, "text/html")]);
        let started = std::time::Instant::now();
        let result = client(&server, None)
            .list_ip_ranges(&ctx, ListIpRangesInput)
            .await;
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(result.unwrap().items, ["192.0.2.0/24"]);
    }
}

fn analytics_range(domain: &str) -> Range {
    let mut range = Range::new(vec![domain.into()]);
    range.period = "24h".into();
    range
}

#[tokio::test]
async fn configured_credentials_never_reach_a_cdn_request_body_or_docs_path() {
    let (ctx, _) = RequestContext::new();
    for canary in [
        CDN_KEY,
        MANAGEMENT,
        "toy%252Dcdn%252Dkey",
        "toy%252Dmanagement%252Dtoken",
    ] {
        let body = r#"{"success":true,"data":"[]"}"#;
        let server = Server::start(vec![response(body, "application/json")]);
        let client = client_with_docs_path(&server, Some(CDN_KEY), "/faq/vcdn");
        let result = client.get_cache_status(&ctx, analytics_range(canary)).await;
        assert_eq!(
            server.requests(),
            0,
            "a credential reached the analytics host"
        );
        assert_eq!(
            result.unwrap_err().error().class(),
            ErrorClass::InvalidInput
        );

        let server = Server::start(vec![response("<html></html>", "text/html")]);
        let path = format!("/faq/{canary}");
        let client = client_with_docs_path(&server, Some(CDN_KEY), &path);
        let result = client.list_ip_ranges(&ctx, ListIpRangesInput).await;
        assert_eq!(server.requests(), 0, "a credential reached the docs host");
        assert_eq!(
            result.unwrap_err().error().class(),
            ErrorClass::InvalidInput
        );
    }
}

#[tokio::test]
async fn public_unauthorized_keeps_status_without_an_auth_error() {
    let server = Server::start(vec![with_status("401 Unauthorized", "", "text/html")]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, Some(CDN_KEY))
        .list_ip_ranges(&ctx, ListIpRangesInput)
        .await
        .unwrap_err();
    let error = failure.error();
    assert_eq!(error.api_error().unwrap().status(), Some(401));
    assert_eq!(error.class(), ErrorClass::Api);
    assert!(!error.has_class(ErrorClass::Auth));
    assert!(!error.api_error().unwrap().retryable());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn oversized_bad_request_preserves_invalid_input() {
    let body = "x".repeat((8 << 20) + 1);
    let server = Server::start(vec![with_status(
        "400 Bad Request",
        &body,
        "application/json",
    )]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server, Some(CDN_KEY))
        .list_web_accelerators(&ctx, ListWebAcceleratorsInput)
        .await
        .unwrap_err();
    let error = failure.error();
    assert_eq!(error.class(), ErrorClass::InvalidInput);
    assert_eq!(error.api_error().unwrap().status(), Some(400));
    assert_eq!(error.api_error().unwrap().code(), "BadRequest");
}

#[tokio::test]
async fn absent_web_accelerator_rule_lists_differ_from_explicit_null() {
    let server = Server::start(vec![
        response(
            r#"{"success":true,"data":{"cdnId":"cdn-1","status":1}}"#,
            "application/json",
        ),
        response(
            r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"childrenRule":null,"childrenAdvanceRule":null}}"#,
            "application/json",
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, Some(CDN_KEY));
    for expected in [serde_json::json!([]), serde_json::Value::Null] {
        let output = client
            .get_web_accelerator(&ctx, GetWebAcceleratorInput::new("cdn-1"))
            .await
            .unwrap();
        let output = serde_json::to_value(output).unwrap();
        assert_eq!(output["WebAccelerator"]["PageRules"], expected);
        assert_eq!(output["WebAccelerator"]["AdvancedRule"], expected);
    }
}

#[tokio::test]
async fn json_text_null_counts_are_preserved_as_null() {
    let body = r#"{"success":true,"data":"null"}"#;
    let server = Server::start(vec![
        response(body, "application/json"),
        response(body, "application/json"),
    ]);
    let (ctx, _) = RequestContext::new();
    let client = client(&server, Some(CDN_KEY));
    let domain = "cdn.example.test";
    let cache = client
        .get_cache_status(&ctx, analytics_range(domain))
        .await
        .unwrap();
    let codes = client
        .get_http_codes(&ctx, analytics_range(domain))
        .await
        .unwrap();
    for result in [serde_json::to_value(cache), serde_json::to_value(codes)] {
        assert_eq!(result.unwrap(), serde_json::json!({"Counts": null}));
    }
}

#[tokio::test]
async fn decoding_limit_withholds_reflections_and_refuses_request_values() {
    let field_echo = r#"{"success":true,"data":[{"domainName":"toy%25252Dcdn%25252Dkey.example.test"},{"domainName":"100%"}]}"#;
    let ok_counts = r#"{"success":true,"data":"[]"}"#;
    let server = Server::start(vec![
        response(field_echo, "application/json"),
        response(ok_counts, "application/json"),
    ]);
    let (ctx, _) = RequestContext::new();
    let cdn = client(&server, Some(CDN_KEY));
    let list = cdn
        .list_web_accelerators(&ctx, ListWebAcceleratorsInput)
        .await
        .unwrap();
    assert!(
        list.items[0].domain_name == "<redacted>",
        "decoder limit exposed a value"
    );
    assert_eq!(list.items[1].domain_name, "100%");
    for encoded in [
        "toy%25252Dmanagement%25252Dtoken",
        r"toy\\\\u002dmanagement\\\\u002dtoken",
    ] {
        let result = cdn.get_cache_status(&ctx, analytics_range(encoded)).await;
        assert_eq!(server.requests(), 1, "decoder limit allowed a request");
        assert_eq!(
            result.unwrap_err().error().class(),
            ErrorClass::InvalidInput
        );
    }
    cdn.get_cache_status(&ctx, analytics_range("100%"))
        .await
        .unwrap();
    assert_eq!(server.requests(), 2);

    let server = Server::start(vec![response("<html></html>", "text/html")]);
    let cdn = client_with_docs_path(
        &server,
        Some(CDN_KEY),
        "/faq/toy%25252Dmanagement%25252Dtoken",
    );
    let result = cdn.list_ip_ranges(&ctx, ListIpRangesInput).await;
    assert_eq!(server.requests(), 0, "decoder limit allowed a docs request");
    assert_eq!(
        result.unwrap_err().error().class(),
        ErrorClass::InvalidInput
    );
}

#[tokio::test]
async fn malformed_end_tags_cannot_expose_hidden_template_content() {
    let (ctx, _) = RequestContext::new();
    for forged_end in [
        "<//template>",
        "</template/>",
        "</template attr='fake >'>",
        "</ template>",
    ] {
        let section = format!(
            "<h3>CDN IP range</h3><template>{forged_end}<p>203.0.113.0/24</p></TeMpLaTe \t><p>192.0.2.0/24</p><h3>Next</h3>"
        );
        let server = Server::start(vec![response(&section, "text/html")]);
        let output = client(&server, None)
            .list_ip_ranges(&ctx, ListIpRangesInput)
            .await
            .unwrap();
        assert_eq!(output.items, ["192.0.2.0/24"], "{forged_end}");
    }
}
