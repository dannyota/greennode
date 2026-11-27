use std::sync::{Arc, atomic::AtomicBool};

use super::{Refresh, client, client_with_token, response, target};
use greennode_sdk::monitor::{
    Client, CreateSyntheticQuotaInput, ErrorDetail, RenewSyntheticQuotaInput,
};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{Config, ConfigOptions, ErrorClass, RequestContext};
use serde_json::{Value, json};

const ABSENT: &str = include_str!("../fixtures/monitor/synthetic_quote_absent.json");
const RENEWAL: &str = include_str!("../fixtures/monitor/synthetic_quote_renewal.json");
const DETAIL: &str = include_str!("../fixtures/monitor/quota_quota_detail.json");
const CLASSES: &str = include_str!("../fixtures/monitor/quota_classes.json");
const UNITS: &str = include_str!("../fixtures/monitor/quota_class_packages.json");
const SETTINGS: &str = include_str!("../fixtures/monitor/quota_settings.json");
const PRICE: &str = include_str!("../fixtures/monitor/synthetic_quote_price.json");
const QUOTE_PATH: &str = "/billing-api/v2/synthetic/prices/created-price";

fn prices(seed: Vec<Response>, units: &str, price: &str) -> Vec<Response> {
    seed.into_iter()
        .chain([CLASSES, units, "[]", SETTINGS, price].map(|body| response("200 OK", body)))
        .collect()
}

fn body(server: &Server, index: usize) -> Value {
    serde_json::from_slice(&server.bodies()[index]).unwrap()
}

#[tokio::test]
async fn create_synthetic_quote_requires_exact_absence_and_posts_only_price_inputs() {
    let server = Server::start(prices(
        vec![response("404 Not Found", ABSENT)],
        UNITS,
        PRICE,
    ));
    let (ctx, _) = RequestContext::new();
    let mut input = CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    input.max_price = f64::NAN;
    input.no_wait = true;
    let output = client(&server)
        .quote_create_synthetic_quota(&ctx, input.clone())
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["PackageID"], "unit_1");
    assert_eq!(json["MonthPeriod"], 1);
    assert!(json["ExpectedEndTime"].is_null());
    assert_eq!(server.requests(), 6);
    let heads = server.heads();
    assert_eq!(target(&heads[0]).path(), "/billing-api/v1/synthetic/quota");
    assert_eq!(target(&heads[5]).path(), QUOTE_PATH);
    assert!(heads[5].starts_with(b"POST "));
    assert!(
        body(&server, 5)
            == json!({"redirectUrl":"", "packageId":"unit_1", "quantity":10, "buyWith":{}})
    );
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"message":"Not found synthetic quota","other":true}"#,
    )]);
    let error = client(&server)
        .quote_create_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::NotFound);
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn renew_synthetic_quote_freezes_expiry_and_sends_resource_identity() {
    let server = Server::start(prices(
        vec![response("200 OK", RENEWAL), response("200 OK", DETAIL)],
        UNITS,
        PRICE,
    ));
    let (ctx, _) = RequestContext::new();
    let mut input = RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z");
    input.max_days_left = -1;
    input.max_price = f64::NAN;
    input.no_wait = true;
    let output = client(&server)
        .quote_renew_synthetic_quota(&ctx, input)
        .await
        .unwrap();
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["ExpectedEndTime"], "2099-02-01T07:00:00+07:00");
    assert_eq!(json["ResourceID"], "quota-1");
    let heads = server.heads();
    assert_eq!(
        target(&heads[1]).path(),
        "/billing-api/v2/synthetic/quota-1/quota-detail"
    );
    assert_eq!(target(&heads[6]).path(), QUOTE_PATH);
    assert!(heads[6].starts_with(b"POST "));
    assert!(
        body(&server, 6)
            == json!({"redirectUrl":"", "packageId":"unit_1", "quantity":10, "resourceId":"quota-1"})
    );
}

#[tokio::test]
async fn renewal_refuses_changed_expiry_and_unknown_status_before_catalog_reads() {
    let (ctx, _) = RequestContext::new();
    for (body, expected_end, detail) in [
        (
            RENEWAL.to_owned(),
            "2099-01-01T00:00:00Z",
            ErrorDetail::QuotaChanged,
        ),
        (
            RENEWAL.replace("ACTIVE", "UNKNOWN"),
            "2099-02-01T00:00:00Z",
            ErrorDetail::UnexpectedStatus,
        ),
    ] {
        let server = Server::start(vec![response("200 OK", &body), response("200 OK", DETAIL)]);
        let error = client(&server)
            .quote_renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new(expected_end))
            .await
            .unwrap_err();
        assert_eq!(error.error().detail::<ErrorDetail>(), Some(&detail));
        assert_eq!(server.requests(), 2);
    }
}

#[tokio::test]
async fn quote_refuses_invalid_price_tokens_and_zero_quote_for_a_paid_offer() {
    let (ctx, _) = RequestContext::new();
    let malformed = [
        r#"{"optimumPrice":{"$serde_json::private::Number":"0"}}"#,
        r#"{"optimumPrice":0,"OptimumPrice":0}"#,
        r#"{"optimumPrice":1e-400}"#,
    ];
    for price in malformed {
        let server = Server::start(prices(
            vec![response("404 Not Found", ABSENT)],
            UNITS,
            price,
        ));
        let input = CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
        let error = client(&server)
            .quote_create_synthetic_quota(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    }
    let paid = UNITS.replace("\"price\": 0", "\"price\": 2");
    let server = Server::start(prices(
        vec![response("404 Not Found", ABSENT)],
        &paid,
        PRICE,
    ));
    let input = CreateSyntheticQuotaInput::new("class-1", "unit_1", 10);
    let error = client(&server)
        .quote_create_synthetic_quota(&ctx, input)
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::Unpriced);
}

#[tokio::test]
async fn quote_refuses_caller_ids_matching_a_configured_credential_before_reading() {
    let server = Server::start(vec![]);
    let (ctx, _) = RequestContext::new();
    let client: Client = client_with_token(&server, "unit_1");
    let error = client
        .quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn quote_refuses_refreshed_bearer_when_body_id_contains_its_base64_form() {
    let package = "prefixYWJjsuffix";
    let classes = CLASSES.replace("unit_1", package);
    let units = UNITS.replace("unit_1", package);
    let server = Server::start(vec![
        response("404 Not Found", ABSENT),
        response("200 OK", &classes),
        response("200 OK", &units),
        response("200 OK", "[]"),
        response("200 OK", SETTINGS),
        response("401 Unauthorized", "{}"),
        response("200 OK", PRICE),
    ]);
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_credentials_provider(Arc::new(Refresh(AtomicBool::new(false))));
    let client = Client::new(
        &Config::new(with_product_loopback(Product::Monitor, options, &server)).unwrap(),
    );
    let (ctx, _) = RequestContext::new();
    let result = client
        .quote_create_synthetic_quota(&ctx, CreateSyntheticQuotaInput::new("class-1", package, 10))
        .await;
    assert!(result.is_err());
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn renewal_refuses_hex_encoded_credentials_in_returned_quota_ids() {
    let quota = RENEWAL.replace("quota-1", "quota-616263");
    let server = Server::start(vec![response("200 OK", &quota)]);
    let (ctx, _) = RequestContext::new();
    let failure = client_with_token(&server, "abc")
        .quote_renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert_eq!(server.requests(), 1);
}

fn redirect(location: &str) -> Response {
    Response::Http {
        status: "307 Temporary Redirect".into(),
        headers: vec![
            ("Location".into(), location.into()),
            ("Content-Length".into(), "0".into()),
        ],
        body: Vec::new(),
    }
}

#[tokio::test]
async fn renewal_reads_follow_redirects() {
    let server = Server::start(vec![
        redirect("/billing-api/v1/synthetic/quota-moved"),
        response("200 OK", RENEWAL),
        response("200 OK", DETAIL),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-01-01T00:00:00Z"))
        .await
        .unwrap_err();
    assert_eq!(
        error.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaChanged)
    );
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn truncated_quote_response_is_invalid_response_without_status() {
    let server = Server::start(vec![
        response("200 OK", RENEWAL),
        response("200 OK", DETAIL),
        response("200 OK", CLASSES),
        response("200 OK", UNITS),
        response("200 OK", "[]"),
        response("200 OK", SETTINGS),
        Response::RawClose(
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n{\"optimumPrice\"".to_vec(),
        ),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.code(), "InvalidResponse");
    assert_eq!(api.status(), None);
}

#[tokio::test]
async fn create_quote_class_message_and_read_first_package_order() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    let error = client(&server)
        .quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("bad_class", "unit_1", 1),
        )
        .await
        .unwrap_err();
    assert_eq!(error.error().class(), ErrorClass::InvalidInput);
    assert_eq!(
        error.error().detail::<String>().map(String::as_str),
        Some(
            "vngcloud: invalid input: monitor.QuoteCreateSyntheticQuota requires ClassID to match ^[A-Za-z0-9-]+$"
        )
    );
    assert_eq!(server.requests(), 0);
    let server = Server::start(prices(
        vec![response("404 Not Found", ABSENT)],
        UNITS,
        PRICE,
    ));
    let error = client(&server)
        .quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "bad package!", 1),
        )
        .await
        .unwrap_err();
    assert!(server.requests() > 1);
    assert!(
        error
            .error()
            .detail::<String>()
            .is_some_and(|text| text.contains("selected synthetic package is unsupported"))
    );
}

#[tokio::test]
async fn create_quote_404_with_invalid_body_says_invalid_structure() {
    let server = Server::start(vec![response("404 Not Found", r#"{"a":1,"a":2}"#)]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_create_synthetic_quota(&ctx, CreateSyntheticQuotaInput::new("class-1", "unit_1", 1))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(404));
    assert_eq!(api.message(), "quota response had invalid structure");
}

#[tokio::test]
async fn create_quote_404_with_non_json_body_is_withheld_not_found() {
    let server = Server::start(vec![response("404 Not Found", "not json")]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_create_synthetic_quota(&ctx, CreateSyntheticQuotaInput::new("class-1", "unit_1", 1))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(404));
    assert_eq!(api.code(), "NotFound");
    assert_eq!(api.message(), "quota response withheld");
}

#[tokio::test]
async fn refused_redirect_keeps_its_status_on_reads_and_quotes() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![redirect("/elsewhere")]);
    let error = client(&server)
        .quote_create_synthetic_quota(&ctx, CreateSyntheticQuotaInput::new("class-1", "unit_1", 1))
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(307));
    assert_eq!(api.code(), "InvalidResponse");
    let mut responses = prices(vec![response("404 Not Found", ABSENT)], UNITS, PRICE);
    responses.pop();
    responses.push(redirect("/elsewhere"));
    let server = Server::start(responses);
    let error = client(&server)
        .quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
        )
        .await
        .unwrap_err();
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(307));
    assert_eq!(api.code(), "InvalidResponse");
}

#[tokio::test]
async fn renewal_catalog_reads_refuse_redirects_but_quota_reads_follow() {
    let server = Server::start(vec![
        redirect("/billing-api/v1/synthetic/quota-moved"),
        response("200 OK", RENEWAL),
        response("200 OK", DETAIL),
        redirect("/elsewhere"),
    ]);
    let (ctx, _) = RequestContext::new();
    let error = client(&server)
        .quote_renew_synthetic_quota(&ctx, RenewSyntheticQuotaInput::new("2099-02-01T00:00:00Z"))
        .await
        .unwrap_err();
    assert_eq!(server.requests(), 4);
    let api = error.error().api_error().unwrap();
    assert_eq!(api.status(), Some(307));
}

#[tokio::test]
async fn create_quote_requires_all_fields_before_any_request() {
    let (ctx, _) = RequestContext::new();
    let server = Server::start(vec![]);
    for (input, field) in [
        (CreateSyntheticQuotaInput::new("", "unit_1", 1), "ClassID"),
        (
            CreateSyntheticQuotaInput::new("class-1", "", 1),
            "PackageID",
        ),
        (
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 0),
            "Quantity",
        ),
    ] {
        let error = client(&server)
            .quote_create_synthetic_quota(&ctx, input)
            .await
            .unwrap_err();
        assert_eq!(
            error.error().detail::<String>().map(String::as_str),
            Some(
                format!(
                    "vngcloud: invalid input: monitor.QuoteCreateSyntheticQuota requires {field}"
                )
                .as_str()
            )
        );
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn cancelled_quote_post_keeps_the_cancelled_class() {
    let (received, request) = tokio::sync::oneshot::channel();
    let mut responses = prices(vec![response("404 Not Found", ABSENT)], UNITS, PRICE);
    responses.pop();
    responses.push(Response::StallBeforeHead(received));
    let server = Server::start(responses);
    let (ctx, cancel) = RequestContext::new();
    let client = client(&server);
    let (result, ()) = tokio::join!(
        client.quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10)
        ),
        async {
            request.await.unwrap();
            cancel.cancel();
        }
    );
    assert!(result.unwrap_err().error().has_class(ErrorClass::Cancelled));
}

#[tokio::test]
async fn deadline_during_quote_post_keeps_the_deadline_class() {
    let (received, request) = tokio::sync::oneshot::channel();
    let mut responses = prices(vec![response("404 Not Found", ABSENT)], UNITS, PRICE);
    responses.pop();
    responses.push(Response::StallBeforeHead(received));
    let server = Server::start(responses);
    let (ctx, _cancel) = RequestContext::new();
    let ctx =
        ctx.child_with_deadline(tokio::time::Instant::now() + std::time::Duration::from_secs(3600));
    let client = client(&server);
    let (result, ()) = tokio::join!(
        client.quote_create_synthetic_quota(
            &ctx,
            CreateSyntheticQuotaInput::new("class-1", "unit_1", 10)
        ),
        async {
            request.await.unwrap();
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(7200)).await;
        }
    );
    assert!(
        result
            .unwrap_err()
            .error()
            .has_class(ErrorClass::DeadlineExceeded)
    );
    assert_eq!(server.requests(), 6);
}

#[tokio::test]
async fn price_post_empty_body_is_invalid_structure_and_whitespace_is_withheld() {
    let (ctx, _) = RequestContext::new();
    for (body, message) in [
        ("", "quota response had invalid structure"),
        (" \n", "quota response withheld"),
    ] {
        let mut responses = prices(vec![response("404 Not Found", ABSENT)], UNITS, PRICE);
        responses.pop();
        responses.push(response("200 OK", body));
        let server = Server::start(responses);
        let error = client(&server)
            .quote_create_synthetic_quota(
                &ctx,
                CreateSyntheticQuotaInput::new("class-1", "unit_1", 10),
            )
            .await
            .unwrap_err();
        let api = error.error().api_error().unwrap();
        assert_eq!(api.code(), "InvalidResponse");
        assert_eq!(api.message(), message);
    }
}
