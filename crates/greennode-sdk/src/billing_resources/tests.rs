use super::*;
use crate::wire::test_server::{Response, Server};
use crate::{Config, ConfigOptions, CredentialsProvider, EndpointOverrides, Token};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const FIRST: &str = "synthetic-first-token";
const SECOND: &str = "synthetic-second-token";
const EMPTY: &str = r#"{"code":200,"data":{"data":[]}}"#;
const FIXTURE: &str = include_str!("../billing/fixtures/ListResources.json");

fn reply(status: &str, body: impl AsRef<[u8]>) -> Response {
    let body = body.as_ref();
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn config(server: &Server, options: ConfigOptions) -> Config {
    let config = Config::new(options.with_region("hcm-3".into()).with_endpoint_overrides(
        EndpointOverrides {
            billing: Some(server.origin.clone()),
            ..Default::default()
        },
    ))
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    config
}

fn static_config(server: &Server) -> Config {
    config(
        server,
        ConfigOptions::default().with_static_token(Secret::new(FIRST)),
    )
}

struct Rotating {
    sent: AtomicUsize,
    values: [&'static str; 2],
}
impl CredentialsProvider for Rotating {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            let index = self.sent.fetch_add(1, Ordering::SeqCst).min(1);
            Ok(Token::new(Secret::new(self.values[index]), None))
        })
    }
    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

fn rotating_config(server: &Server, values: [&'static str; 2]) -> Config {
    config(
        server,
        ConfigOptions::default().with_credentials_provider(Arc::new(Rotating {
            sent: AtomicUsize::new(0),
            values,
        })),
    )
}

fn refusal<T, E>(result: Result<T, E>) -> E {
    let Err(error) = result else {
        panic!("expected billing refusal");
    };
    error
}

fn invalid_at(error: &Error, status: u16) {
    let detail = error.api_error().unwrap();
    assert!(detail.code() == "InvalidResponse");
    assert_eq!(detail.status(), Some(status));
}

fn assert_no_echo(error: &Error, values: &[&str]) {
    let detail = error.api_error().unwrap();
    assert!(values.iter().all(|value| !detail.message().contains(value)
        && !detail.code().contains(value)
        && !format!("{error:?} {error}").contains(value)));
}

fn setting() -> Setting<'static> {
    Setting {
        product: "vstorage",
        artifact_type: "object-storage",
        artifact_id: "resource-1",
        channel: 0,
        auto_renew_info: AutoRenewInfo {
            is_enable: true,
            period: 43_200,
        },
    }
}

#[tokio::test]
async fn resource_successful_credential_echo_withholds_each_typed_and_raw_field() {
    for credential in [FIRST, SECOND] {
        for field in [
            "artifactName",
            "artifactId",
            "status",
            "statusUI",
            "billingElements",
            "extra",
        ] {
            let data = match field {
                "extra" => json!({"data":[], "extra":{"warningThresholds":credential}}),
                "status" | "statusUI" => json!({"data":[{(field):{"nested":credential}}]}),
                "billingElements" => {
                    json!({"data":[{"billingElements":[{"quantity":credential}]}]})
                }
                _ => json!({"data":[{(field):credential}]}),
            };
            let body = json!({"code":200,"data":data})
                .to_string()
                .replace("synthetic", &format!("{}u0073ynthetic", char::from(92)));
            let server = Server::start(vec![reply("401 Unauthorized", ""), reply("200 OK", &body)]);
            let config = rotating_config(&server, [FIRST, SECOND]);
            let (ctx, _) = RequestContext::new();
            let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
            invalid_at(&error, 200);
            assert_eq!(error.api_error().unwrap().message(), INVALID);
            assert!(error.detail::<StatusCode>().is_none());
            assert_no_echo(&error, &[FIRST, SECOND]);
            let heads = server.heads();
            assert_eq!(heads.len(), 2);
            assert!(
                heads
                    .iter()
                    .all(|head| head.starts_with(b"GET /gateway/api/v1/resources "))
            );
        }
    }
}

#[tokio::test]
async fn unexpected_success_and_empty_ok_are_invalid_responses() {
    for (status, body, expected_status, message) in [
        ("204 No Content", "", 204, WITHHELD),
        ("201 Created", "", 201, WITHHELD),
        ("200 OK", "", 200, INVALID),
    ] {
        let server = Server::start(vec![reply(status, body)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        invalid_at(&error, expected_status);
        assert_eq!(error.api_error().unwrap().message(), message);
    }
}

#[tokio::test]
async fn malformed_and_truncated_resources_keep_status_and_only_the_safe_read_detail() {
    for truncated in [false, true] {
        let response = if truncated {
            Response::RawClose(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"code\":".to_vec())
        } else {
            reply("200 OK", br#"{"code":"#)
        };
        let server = Server::start(vec![response]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        let failure = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        invalid_at(&failure, 200);
        assert_eq!(failure.api_error().unwrap().message(), WITHHELD);
        assert_eq!(
            failure.detail::<StatusCode>().copied(),
            truncated.then_some(StatusCode::OK)
        );
    }
}

#[tokio::test]
async fn decoded_resource_cost_reflection_is_checked_against_earlier_bearers() {
    let server = Server::start(vec![
        reply("401 Unauthorized", ""),
        reply(
            "200 OK",
            r#"{"code":200,"data":{"data":[{"artifactId":"resource-1","cost":1e6}]}}"#,
        ),
    ]);
    let config = rotating_config(&server, ["e+06", SECOND]);
    let (ctx, _) = RequestContext::new();
    let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
    invalid_at(&error, 200);
    assert_no_echo(&error, &["e+06", SECOND]);
    assert_eq!(server.heads().len(), 2);
}

#[tokio::test]
async fn resource_list_refuses_duplicate_fields_including_raw_nested_and_escaped_keys() {
    for (from, replacement) in [
        (
            r#""isRenewing": false"#,
            r#""isRenewing":true,"isRenewing":null"#,
        ),
        (r#""code": 200"#, r#""code":500,"code":200"#),
        (r#""data": {"#, r#""data":null,"data": {"#),
        (r#""level": "good""#, r#""level":"bad","level":"good"#),
        (
            r#""isRenewing": false"#,
            r#""isRenewing":true,"is@u0052enewing":false"#,
        ),
    ] {
        let replacement = replacement.replace('@', &char::from(92).to_string());
        let body = FIXTURE.replacen(from, &replacement, 1);
        assert!(body != FIXTURE);
        let server = Server::start(vec![reply("200 OK", &body)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        invalid_at(
            &list(config.session(), &ctx, &mut Evidence::new())
                .await
                .unwrap_err(),
            200,
        );
    }
}

#[tokio::test]
async fn account_refuses_duplicate_alias_missing_and_disagreeing_identifiers() {
    for fields in [
        r#""accountId":12345,"userId":54321,"userId":12345"#,
        r#""accountId":54321,"accountId":12345,"userId":12345"#,
        r#""accountId":111,"AccountId":123456"#,
        r#""UserId":12345"#,
        r#""accountId":12345,"userId":54321"#,
        "",
    ] {
        let body = format!(r#"{{"code":200,"data":{{{fields}}}}}"#);
        let server = Server::start(vec![reply("200 OK", &body)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        invalid_at(
            &account(config.session(), &ctx, &mut Evidence::new())
                .await
                .err()
                .unwrap(),
            200,
        );
    }
}

#[tokio::test]
async fn projection_field_names_do_not_trigger_credential_refusal() {
    let server = Server::start(vec![reply("200 OK", EMPTY)]);
    let config = config(
        &server,
        ConfigOptions::default().with_static_token(Secret::new("Items")),
    );
    let (ctx, _) = RequestContext::new();
    assert!(
        list(config.session(), &ctx, &mut Evidence::new())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn resource_list_accepts_sibling_keys_and_equivalent_numeric_success_code() {
    let server = Server::start(vec![
        reply(
            "200 OK",
            r#"{"code":200,"data":{"data":[{"status":{"a":1},"statusUI":{"a":2}},{"status":{"a":3}}]}}"#,
        ),
        reply("200 OK", r#"{"code":200.0,"data":{"data":[]}}"#),
    ]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    let output = list(config.session(), &ctx, &mut evidence).await.unwrap();
    assert_eq!(output.projection.items.len(), 2);
    assert!(
        list(config.session(), &ctx, &mut evidence)
            .await
            .unwrap()
            .projection
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn overlapping_bearers_and_malformed_types_do_not_retain_peer_errors() {
    let first = "synthetic-shared-token";
    let second = "synthetic-shared-token-SECRET-TAIL";
    for order in [[first, second], [second, first]] {
        for body in [
            json!({"code":second,"message":second}).to_string(),
            r#"{"code":200,"data":{"data":[{"cost":123456789e999}]}}"#.into(),
        ] {
            let server = Server::start(vec![reply("401 Unauthorized", ""), reply("200 OK", &body)]);
            let config = rotating_config(&server, order);
            let (ctx, _) = RequestContext::new();
            let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
            invalid_at(&error, 200);
            assert_eq!(error.api_error().unwrap().message(), INVALID);
            assert!(error.detail::<StatusCode>().is_none());
            assert_no_echo(&error, &[first, second, "123456789e999"]);
            assert_eq!(server.heads().len(), 2);
        }
    }
}

#[tokio::test]
async fn earlier_bearers_are_withheld_on_success_and_error_after_refresh() {
    let body = json!({"code":FIRST,"message":format!("bad {FIRST} {SECOND}")}).to_string();
    for (http_status, status, code) in [
        ("200 OK", 200, "InvalidResponse"),
        ("400 Bad Request", 400, "BadRequest"),
    ] {
        let server = Server::start(vec![
            reply("401 Unauthorized", ""),
            reply(http_status, &body),
        ]);
        let config = rotating_config(&server, [FIRST, SECOND]);
        let (ctx, _) = RequestContext::new();
        let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
        assert_eq!(error.api_error().unwrap().status(), Some(status));
        assert_eq!(error.api_error().unwrap().code(), code);
        assert_no_echo(&error, &[FIRST, SECOND]);
        assert_eq!(server.heads().len(), 2);
    }
}

#[tokio::test]
async fn account_uses_an_exact_positive_integer_and_sends_no_account_header() {
    for field in ["accountId", "userId"] {
        for (input, expected) in [
            ("12345", Some("12345")),
            ("12345.0", Some("12345")),
            ("9223372036854775807", Some("9223372036854775807")),
            ("0", None),
            ("-1", None),
            ("1.5", None),
            ("null", None),
            (r#""12345""#, None),
            ("9223372036854775808", None),
        ] {
            let body = format!(r#"{{"code":200,"data":{{"{field}":{input}}}}}"#);
            let server = Server::start(vec![reply("200 OK", &body)]);
            let config = static_config(&server);
            let (ctx, _) = RequestContext::new();
            let result = account(config.session(), &ctx, &mut Evidence::new()).await;
            if let Some(expected) = expected {
                assert!(result.unwrap().0.expose_secret() == expected);
            } else {
                invalid_at(&result.err().unwrap(), 200);
            }
            let heads = server.heads();
            assert_eq!(heads.len(), 1);
            assert!(heads[0].starts_with(b"GET /gateway/api/v1/home/user-info "));
            assert!(!String::from_utf8_lossy(&heads[0]).contains("portal-user-id"));
        }
    }
}

#[tokio::test]
async fn resources_refuse_invalid_types_and_lone_aliases() {
    for body in [
        r#"{"code":200,"Code":200,"data":{"data":[]}}"#,
        r#"{"code":200,"data":{"data":[]},"Data":{"data":[]}}"#,
        r#"{"code":200,"data":{"Data":[]}}"#,
        r#"{"code":200,"data":{"data":[{"artifactId":"nat-1","ArtifactId":"nat-2"}]}}"#,
        r#"{"code":200,"data":{"data":[{"billingElements":[{"sku":"nat.s-standard","SKU":"other"}]}]}}"#,
        r#"{"code":200,"data":{"data":[null]}}"#,
        r#"{"code":200,"data":{"data":[{"cost":"NaN"}]}}"#,
        r#"{"code":200,"data":{"data":[],"totalAutoRenews":"1"}}"#,
        r#"{"code":200,"data":{"data":[{"billingElements":[{"sku":1}]}]}}"#,
    ] {
        let server = Server::start(vec![reply("200 OK", body)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        invalid_at(
            &list(config.session(), &ctx, &mut Evidence::new())
                .await
                .unwrap_err(),
            200,
        );
    }
}

#[tokio::test]
async fn invalid_utf8_expansion_does_not_count_toward_wire_cap() {
    let mut body = br#"{"code":200,"data":{"data":[],"padding":""#.to_vec();
    body.extend(std::iter::repeat_n(0xff, (CAP / 2) + 100));
    body.extend_from_slice(br#""}}"#);
    assert!(body.len() < CAP);
    let server = Server::start(vec![reply("200 OK", body)]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    assert!(
        list(config.session(), &ctx, &mut Evidence::new())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn oversized_billing_response_uses_the_go_status_precedence() {
    let body = format!(
        r#"{{"code":200,"data":{{"data":[],"padding":"{}"}}}}"#,
        "x".repeat(5 << 20)
    );
    let server = Server::start(vec![
        reply("200 OK", &body),
        reply("400 Bad Request", &body),
    ]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
    invalid_at(&error, 200);
    assert!(error.is_body_too_large());
    let error = refusal(list(config.session(), &ctx, &mut Evidence::new()).await);
    assert_eq!(error.api_error().unwrap().code(), "BadRequest");
    assert_eq!(error.api_error().unwrap().status(), Some(400));
}

#[tokio::test]
async fn put_refuses_short_embedded_identifier_echoes_after_bearer_rotation() {
    for account_matches in [true, false] {
        let id = if account_matches { 13031329 } else { 123456 };
        let account_body = json!({"code":200,"data":{"accountId":id}}).to_string();
        let server = Server::start(vec![
            reply("200 OK", account_body),
            reply(
                "200 OK",
                r#"{"code":200,"data":{"successAll":true,"errorAutoRenewResources":[]}}"#,
            ),
        ]);
        let config = rotating_config(&server, [FIRST, "012"]);
        let (ctx, _) = RequestContext::new();
        let mut evidence = Evidence::new();
        let account = account(config.session(), &ctx, &mut evidence)
            .await
            .unwrap();
        let mut input = setting();
        if !account_matches {
            input.artifact_id = "13031329";
        }
        let result = put(
            config.session(),
            &ctx,
            "put-test",
            &account,
            input,
            &mut evidence,
        )
        .await;
        let heads = server.heads();
        assert_eq!(heads.len(), 1, "the rotated bearer must prevent the PUT");
        assert!(heads[0].starts_with(b"GET /gateway/api/v1/home/user-info "));
        let failure = refusal(result);
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(failure.write_dispatch(), WriteDispatch::NotSent);
    }
}

#[tokio::test]
async fn put_refuses_duplicate_and_aliased_response_fields() {
    for body in [
        r#"{"code":200,"data":{"successAll":false,"SuccessAll":true,"errorAutoRenewResources":[{}],"ErrorAutoRenewResources":[]}}"#,
        r#"{"code":200,"data":{"successAll":false,"successAll":true,"errorAutoRenewResources":[]}}"#,
        r#"{"code":200,"data":{"successAll":true,"errorAutoRenewResources":null}}"#,
    ] {
        let server = Server::start(vec![reply("200 OK", body)]);
        let config = static_config(&server);
        let (ctx, _) = RequestContext::new();
        let failure = refusal(
            put(
                config.session(),
                &ctx,
                "put-test",
                &AccountId(Secret::new("123456")),
                setting(),
                &mut Evidence::new(),
            )
            .await,
        );
        invalid_at(failure.error(), 200);
        assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    }
}

#[tokio::test]
async fn put_sends_one_numeric_account_header_and_keeps_safe_rejections_and_dispatch() {
    let account_body = r#"{"code":200,"data":{"accountId":123456.0,"userId":123456}}"#;
    let rejected = r#"{"code":200,"data":{"successAll":true,"errorAutoRenewResources":[{"message":"private renewal detail"}]}}"#;
    let accepted = r#"{"code":200,"data":{"successAll":true,"errorAutoRenewResources":[]}}"#;
    let server = Server::start(vec![
        reply("200 OK", account_body),
        reply("200 OK", rejected),
        reply("200 OK", accepted),
        reply("401 Unauthorized", ""),
    ]);
    let config = rotating_config(&server, [FIRST, SECOND]);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    let account = account(config.session(), &ctx, &mut evidence)
        .await
        .unwrap();
    let failure = put(
        config.session(),
        &ctx,
        "put-test",
        &account,
        setting(),
        &mut evidence,
    )
    .await
    .unwrap_err();
    let detail = failure.error().api_error().unwrap();
    assert_eq!(detail.code(), "AutoRenewRejected");
    assert_eq!(
        detail.message(),
        "server refused auto-renew setting; error count 1"
    );
    assert_eq!(failure.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    put(
        config.session(),
        &ctx,
        "put-test",
        &account,
        setting(),
        &mut evidence,
    )
    .await
    .unwrap();
    let refused = put(
        config.session(),
        &ctx,
        "put-test",
        &account,
        setting(),
        &mut evidence,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.error().api_error().unwrap().status(), Some(401));
    assert_eq!(refused.write_dispatch(), WriteDispatch::MayHaveBeenSent);
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    for head in heads.iter().skip(1) {
        assert!(head.starts_with(b"PUT /gateway/api/v1/resources/autoRenew "));
        assert!(String::from_utf8_lossy(head).contains("portal-user-id: 123456"));
    }
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[1]).unwrap();
    assert!(
        body == json!([{"product":"vstorage","artifactType":"object-storage","artifactId":"resource-1","channel":0,"autoRenewInfo":{"isEnable":true,"period":43200}}])
    );
}

#[tokio::test]
async fn account_uses_caller_evidence_and_screens_the_rendered_identity() {
    let server = Server::start(vec![
        reply("200 OK", EMPTY),
        reply("200 OK", r#"{"code":200,"data":{"accountId":1.2345e4}}"#),
    ]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let mut evidence = Evidence::new();
    evidence.add_secret(&Secret::new("12345"));
    list(config.session(), &ctx, &mut evidence).await.unwrap();
    let error = account(config.session(), &ctx, &mut evidence)
        .await
        .err()
        .unwrap();
    invalid_at(&error, 200);
    assert!(evidence.text_reflects(FIRST));
    assert_no_echo(&error, &[FIRST, "12345"]);
}

#[tokio::test]
async fn negative_zero_is_accepted_for_all_integer_fields() {
    let server = Server::start(vec![reply(
        "200 OK",
        r#"{"code":200,"data":{"data":[{"renewPeriod":-0,"channel":-0,"startBillingTime":-0,"endBillingTime":-0,"dateLeft":-0}],"totalAutoRenews":-0,"totalGoodResources":-0,"totalWarningResources":-0,"totalAlertedResources":-0,"totalExpiredResources":-0}}"#,
    )]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let resources = list(config.session(), &ctx, &mut Evidence::new())
        .await
        .unwrap();
    let evidence = Evidence::new();
    let (row, period) = resources.rows(&evidence).unwrap().next().unwrap();
    assert_eq!(period, RenewPeriod::Value(0));
    assert_eq!(
        (
            row.channel,
            row.start_billing_time,
            row.end_billing_time,
            row.date_left
        ),
        (Some(0), Some(0), Some(0), Some(0))
    );
    assert_eq!(
        (
            resources.projection.total_auto_renews,
            resources.projection.total_good_resources,
            resources.projection.total_warning_resources,
            resources.projection.total_alerted_resources,
            resources.projection.total_expired_resources
        ),
        (Some(0), Some(0), Some(0), Some(0), Some(0))
    );
}

#[tokio::test]
async fn shared_rows_distinguish_null_omitted_and_numeric_renewal_periods() {
    let server = Server::start(vec![reply(
        "200 OK",
        r#"{"code":200,"data":{"data":[{"artifactId":"nat-1","renewPeriod":null},{"artifactId":"nat-2"},{"artifactId":"nat-3","renewPeriod":1}]}}"#,
    )]);
    let config = static_config(&server);
    let (ctx, _) = RequestContext::new();
    let resources = list(config.session(), &ctx, &mut Evidence::new())
        .await
        .unwrap();
    let evidence = Evidence::new();
    let periods: Vec<_> = resources
        .rows(&evidence)
        .unwrap()
        .map(|(resource, period)| (resource.artifact_id.as_str(), period))
        .collect();
    assert_eq!(
        periods,
        [
            ("nat-1", RenewPeriod::Null),
            ("nat-2", RenewPeriod::Omitted),
            ("nat-3", RenewPeriod::Value(1))
        ]
    );
    let rendered = serde_json::to_value(&resources.projection).unwrap();
    assert!(rendered["Items"][0]["RenewPeriod"].is_null());
    assert!(rendered["Items"][1]["RenewPeriod"].is_null());
    assert_eq!(rendered["Items"][2]["RenewPeriod"], 1);
}

mod guarded_responses;
mod response_decoding;
