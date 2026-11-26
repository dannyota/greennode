#![cfg(feature = "test-support")]

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
    let mut options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new(MANAGEMENT))
        .with_endpoint_overrides(EndpointOverrides {
            cdn_docs: Some(format!("{}/faq/vcdn", server.origin)),
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
