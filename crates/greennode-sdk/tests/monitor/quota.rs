use super::{client, client_with_token, response, target};
use greennode_sdk::monitor::{ErrorDetail, GetQuotaStatusInput, ListSyntheticPackagesInput};
use greennode_sdk::testing::Server;
use greennode_sdk::{ErrorClass, RequestContext, WriteDispatch};

const CLASSES: &str = include_str!("../fixtures/monitor/quota_classes.json");
const UNITS: &str = include_str!("../fixtures/monitor/quota_class_packages.json");
const PACKAGES: &str = include_str!("../fixtures/monitor/quota_v1_packages.json");
const SETTINGS: &str = include_str!("../fixtures/monitor/quota_settings.json");
const QUOTA: &str = include_str!("../fixtures/monitor/quota_synthetic.json");
const DETAIL: &str = include_str!("../fixtures/monitor/quota_quota_detail.json");

#[tokio::test]
async fn synthetic_catalog_preserves_distinct_ids_prices_and_periods() {
    let server = Server::start(
        [CLASSES, PACKAGES, SETTINGS, UNITS, "[]"]
            .map(|body| response("200 OK", body))
            .into(),
    );
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .list_synthetic_packages(&ctx, ListSyntheticPackagesInput)
        .await
        .unwrap();
    let output = serde_json::to_value(output).unwrap();
    assert_eq!(
        output["Items"][0]["Options"][0]["MonthPeriods"],
        serde_json::json!([1])
    );
    assert_eq!(
        output["Items"][0]["Options"][0]["Price"],
        serde_json::Value::Null
    );
    assert_eq!(output["Items"][0]["Packages"][0]["ID"], "unit_1");
    assert_eq!(output["Packages"][0]["ID"], "v1_1");
    assert_eq!(output["Packages"][1]["MonthPeriods"], serde_json::json!([]));
    let requests: Vec<_> = server
        .heads()
        .iter()
        .map(|head| target(head).path().to_owned())
        .collect();
    assert_eq!(
        requests,
        [
            "/billing-api/v2/synthetic/quota-class",
            "/billing-api/v1/synthetic/packages",
            "/billing-api/v1/settings",
            "/billing-api/v2/synthetic/quota-class/class-1/packages",
            "/billing-api/v2/synthetic/quota-class/class-2/packages"
        ]
    );
}

#[tokio::test]
async fn synthetic_catalog_refuses_aliased_fields_and_preflights_peer_class_ids() {
    let (ctx, _) = RequestContext::new();
    let malformed = CLASSES.replace("\"priority\": 1.0", "\"priority\": 1.0, \"Priority\": 1");
    let server = Server::start(vec![response("200 OK", &malformed)]);
    let error = client(&server)
        .list_synthetic_packages(&ctx, ListSyntheticPackagesInput)
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.requests(), 1);
    let unsafe_id = CLASSES.replace("class-2", "unsafe/class");
    let server = Server::start(
        [unsafe_id.as_str(), PACKAGES, SETTINGS]
            .map(|body| response("200 OK", body))
            .into(),
    );
    let error = client(&server)
        .list_synthetic_packages(&ctx, ListSyntheticPackagesInput)
        .await
        .unwrap_err();
    assert_eq!(error.error().api_error().unwrap().code(), "InvalidResponse");
    assert_eq!(server.requests(), 3);
}

#[tokio::test]
async fn quota_status_preserves_partial_rows_and_only_treats_source_404_as_none() {
    let server = Server::start(vec![
        response("200 OK", QUOTA),
        response("404 Not Found", "{}"),
        response("404 Not Found", "{}"),
        response("403 Forbidden", r#"{"message":"peer-canary"}"#),
    ]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server)
        .get_quota_status(&ctx, GetQuotaStatusInput::default())
        .await
        .unwrap_err();
    assert_eq!(result.write_dispatch(), WriteDispatch::NotAWrite);
    assert_eq!(
        result.error().detail::<ErrorDetail>(),
        Some(&ErrorDetail::QuotaStatusIncomplete)
    );
    assert!(result.error().has_class(ErrorClass::PermissionDenied));
    let output = serde_json::to_value(result.partial().unwrap()).unwrap();
    assert_eq!(output["Complete"], false);
    assert_eq!(output["Items"][0]["Error"]["Code"], "NotFound");
    assert_eq!(output["Items"][1]["Status"], "none");
    assert_eq!(output["Items"][2]["Error"]["Status"], 403);
    assert!(!output.to_string().contains("peer-canary"));
    assert_eq!(server.requests(), 4);
}

#[tokio::test]
async fn quota_status_reads_free_eligibility_in_the_baseline_order() {
    let server = Server::start(
        [QUOTA, DETAIL, CLASSES, UNITS, "[]", SETTINGS]
            .map(|body| response("200 OK", body))
            .into(),
    );
    let (ctx, _) = RequestContext::new();
    let output = client(&server)
        .get_quota_status(
            &ctx,
            GetQuotaStatusInput {
                kind: "sYnThEtIc".into(),
            },
        )
        .await
        .unwrap();
    let output = serde_json::to_value(output).unwrap();
    assert_eq!(output["Items"][0]["RenewWindowOpen"], false);
    assert_eq!(output["Items"][0]["Price"], 0.0);
    let requests: Vec<_> = server
        .heads()
        .iter()
        .map(|head| target(head).path().to_owned())
        .collect();
    assert_eq!(
        requests,
        [
            "/billing-api/v1/synthetic/quota",
            "/billing-api/v2/synthetic/quota-1/quota-detail",
            "/billing-api/v2/synthetic/quota-class",
            "/billing-api/v2/synthetic/quota-class/class-1/packages",
            "/billing-api/v2/synthetic/quota-class/class-2/packages",
            "/billing-api/v1/settings"
        ]
    );
}

#[tokio::test]
async fn quota_catalog_refuses_reflected_catalog_ids_before_projection() {
    let server = Server::start(
        [CLASSES, PACKAGES, SETTINGS, UNITS, "[]"]
            .map(|body| response("200 OK", body))
            .into(),
    );
    let (ctx, _) = RequestContext::new();
    let failure = client_with_token(&server, "unit_1")
        .list_synthetic_packages(&ctx, ListSyntheticPackagesInput)
        .await
        .unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
    assert_eq!(server.requests(), 5);
}

#[tokio::test]
async fn oversized_quota_error_keeps_http_status_and_withholds_body() {
    let oversized = "x".repeat((1 << 20) + 1);
    let server = Server::start(vec![response("403 Forbidden", &oversized)]);
    let (ctx, _) = RequestContext::new();
    let failure = client(&server)
        .list_synthetic_packages(&ctx, ListSyntheticPackagesInput)
        .await
        .unwrap_err();
    assert_eq!(failure.error().class(), ErrorClass::PermissionDenied);
    let api = failure.error().api_error().unwrap();
    assert_eq!(
        (api.status(), api.code(), api.message()),
        (Some(403), "Forbidden", "quota response withheld")
    );
    assert_eq!(server.requests(), 1);
}
