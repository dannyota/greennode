use super::*;

const TRANSACTIONS: &[u8] = include_bytes!("../fixtures/ListTransactions.json");
const PAYMENTS: &[u8] = include_bytes!("../fixtures/ListPayments.json");
const RESOURCES: &[u8] = include_bytes!("../fixtures/ListResources.json");

#[tokio::test]
async fn sends_history_dates_and_clamps_size() {
    let server = Server::start(vec![response("200 OK", TRANSACTIONS)]);
    let (ctx, _) = RequestContext::new();
    let mut input = ListTransactionsInput::new("2024-11-01");
    input.end_date = "2024-11-30".into();
    input.page = -1;
    input.size = 51;
    let page = client(&server, BEARER)
        .list_transactions(&ctx, input)
        .await
        .unwrap();
    assert!(server.heads()[0].starts_with(
        b"GET /gateway/api/v1/transactions?dateEnd=01%2F12%2F2024&dateStart=01%2F11%2F2024&pageNumber=1&pageSize=50 "
    ));
    assert_eq!(page.page, 1);
    assert!(!page.items.is_empty());
}

#[tokio::test]
async fn payment_walk_refuses_repeated_coverage() {
    let server = Server::start(vec![
        response("200 OK", PAYMENTS),
        response("200 OK", PAYMENTS),
    ]);
    let (ctx, _) = RequestContext::new();
    let mut input = ListPaymentsInput::new("2024-11-01");
    input.end_date = "2024-11-30".into();
    input.all = true;
    let failure = client(&server, BEARER)
        .list_payments(&ctx, input)
        .await
        .unwrap_err();
    let (error, partial, _) = failure.into_parts();
    assert_eq!(error.class(), ErrorClass::HistoryIncomplete);
    assert!(partial.is_none());
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn history_errors_keep_safe_status_and_no_partial_report() {
    for (status, expected) in [
        ("201 Created", "InvalidResponse"),
        ("302 Found", "InvalidResponse"),
        ("400 Bad Request", "BadRequest"),
        ("401 Unauthorized", "Unauthorized"),
        ("403 Forbidden", "Forbidden"),
        ("404 Not Found", "NotFound"),
        ("418 I'm a teapot", "ClientError"),
    ] {
        let body = br#"{"code":"synthetic-peer-canary","message":"synthetic-peer-canary"}"#;
        let server = Server::start(vec![response(status, body)]);
        let (ctx, _) = RequestContext::new();
        let mut input = ListPaymentsInput::new("2024-11-01");
        input.end_date = "2024-11-30".into();
        let (error, partial, _) = client(&server, BEARER)
            .list_payments(&ctx, input)
            .await
            .unwrap_err()
            .into_parts();
        assert!(partial.is_none());
        assert_eq!(error.api_error().unwrap().code(), expected);
        assert_eq!(
            error.api_error().unwrap().status(),
            status[..3].parse().ok()
        );
        assert!(!format!("{error:?} {error}").contains("synthetic-peer-canary"));
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn resources_lists_renewal_state_without_paging() {
    let server = Server::start(vec![response("200 OK", RESOURCES)]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .list_resources(&ctx, ListResourcesInput {})
        .await
        .unwrap();
    assert!(server.heads()[0].starts_with(b"GET /gateway/api/v1/resources "));
    assert_eq!(output.items.len(), 3);
    assert_eq!(output.total_auto_renews, Some(1));
    assert_eq!(output.items[0].renew_period, None);
    assert_eq!(output.items[0].is_renewing, Some(false));
    let encoded = serde_json::to_value(output).unwrap();
    let item = &encoded["Items"][0];
    assert_eq!(item["ArtifactID"], "project-1");
    assert_eq!(item["BillingElements"][0]["SKU"], "synthetic-sku");
    assert_eq!(item["Cost"].as_f64(), Some(10_000_000.123_456_79));
    assert_eq!(item["StatusUI"]["level"], "good");
}

fn page_body(number: i64, total: i64, content: &str) -> String {
    format!(
        r#"{{"code":"200","data":{{"content":[{content}],"pageable":{{"number":{number},"size":1,"totalPages":{total},"totalElements":{total}}}}}}}"#
    )
}

fn payment_row(id: i64, description: &str) -> String {
    let description = serde_json::to_string(description).unwrap();
    format!(
        r#"{{"paymentID":{id},"transactionID":"t-1","payGate":"new-gateway","paymentStatus":"pending","paymentAmount":0,"paymentType":9,"description":{description},"createTimeStamp":"2024-11-11T18:00:00.123456789+00:00"}}"#
    )
}

fn payments() -> ListPaymentsInput {
    let mut input = ListPaymentsInput::new("2024-11-01");
    input.end_date = "2024-11-30".into();
    input.size = 1;
    input
}

#[tokio::test]
async fn invalid_history_range_and_all_page_fail_before_sending() {
    let server = Server::start(Vec::new());
    let (ctx, _) = RequestContext::new();
    let client = client(&server, BEARER);
    for (start, end, page) in [
        ("", "2024-11-02", 1),
        ("2024-02-30", "2024-03-01", 1),
        ("2024-11-02", "2024-11-01", 1),
        ("9999-12-31", "9999-12-31", 1),
        ("2024-11-01", "2024-11-02", 2),
    ] {
        let input = ListPaymentsInput {
            start_date: start.into(),
            end_date: end.into(),
            all: true,
            page,
            size: 1,
        };
        let (error, partial, _) = client
            .list_payments(&ctx, input)
            .await
            .unwrap_err()
            .into_parts();
        assert_eq!(error.class(), ErrorClass::InvalidInput);
        assert!(partial.is_none());
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn transaction_single_page_keeps_totals_and_nullable_values() {
    let server = Server::start(vec![response(
        "200 OK",
        include_bytes!("../fixtures/ListTransactionsPage2.json"),
    )]);
    let (ctx, _) = RequestContext::new();
    let output = client(&server, BEARER)
        .list_transactions(
            &ctx,
            ListTransactionsInput {
                end_date: "2024-11-30".into(),
                page: 2,
                size: 10,
                ..ListTransactionsInput::new("2024-11-01")
            },
        )
        .await
        .unwrap();
    assert!(server.heads()[0].starts_with(
        b"GET /gateway/api/v1/transactions?dateEnd=01%2F12%2F2024&dateStart=01%2F11%2F2024&pageNumber=2&pageSize=10 "
    ));
    assert_eq!(
        (
            output.page,
            output.page_size,
            output.total_page,
            output.total_item
        ),
        (2, 10, 13, 130)
    );
    assert_eq!(output.items[0].type_code, 2);
    assert_eq!(output.items[0].r#type, "REFUND");
    assert!(output.items[0].source_reference.is_none());
}

#[tokio::test]
async fn walk_validates_coverage_before_rows() {
    let server = Server::start(vec![
        response(
            "200 OK",
            page_body(1, 2, &payment_row(1, "ordinary")).as_bytes(),
        ),
        response("200 OK", page_body(2, 3, "{}").as_bytes()),
    ]);
    let (ctx, _) = RequestContext::new();
    let (error, partial, _) = client(&server, BEARER)
        .list_payments(
            &ctx,
            ListPaymentsInput {
                all: true,
                ..payments()
            },
        )
        .await
        .unwrap_err()
        .into_parts();
    assert_eq!(error.class(), ErrorClass::HistoryIncomplete);
    assert!(partial.is_none());
}

#[tokio::test]
async fn walks_refuse_duplicates_and_excessive_page_counts() {
    let row = payment_row(1, "ordinary");
    for replies in [
        vec![page_body(1, 2, &row), page_body(2, 2, &row)],
        vec![page_body(1, 101, &row)],
    ] {
        let count = replies.len();
        let server = Server::start(
            replies
                .iter()
                .map(|body| response("200 OK", body.as_bytes()))
                .collect(),
        );
        let (ctx, _) = RequestContext::new();
        let (error, partial, _) = client(&server, BEARER)
            .list_payments(
                &ctx,
                ListPaymentsInput {
                    all: true,
                    ..payments()
                },
            )
            .await
            .unwrap_err()
            .into_parts();
        assert_eq!(error.class(), ErrorClass::HistoryIncomplete);
        assert!(partial.is_none());
        assert_eq!(server.requests(), count);
    }
}

#[tokio::test]
async fn walk_sanitizes_previous_rows_with_later_credentials() {
    use crate::{CredentialsProvider, Token};
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
    };
    const LATER: &str = "future-canary-ŋ";

    struct Rotating(AtomicUsize);
    impl CredentialsProvider for Rotating {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
            let value = if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
                "initial-canary"
            } else {
                LATER
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

    let first = payment_row(1, &format!("prefix {} suffix", LATER.to_uppercase()));
    let second = payment_row(2, "ordinary");
    let server = Server::start(vec![
        response("200 OK", page_body(1, 2, &first).as_bytes()),
        response("200 OK", page_body(2, 2, &second).as_bytes()),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_credentials_provider(Arc::new(Rotating(AtomicUsize::new(0))))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let output = Client::new(&config)
        .list_payments(
            &ctx,
            ListPaymentsInput {
                all: true,
                ..payments()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (
            output.page,
            output.page_size,
            output.total_page,
            output.total_item
        ),
        (1, 2, 1, 2)
    );
    assert_eq!(
        output.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(output.items[0].description.as_deref() == Some("prefix [redacted] suffix"));
    assert!(output.items[1].description.as_deref() == Some("ordinary"));
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn history_rejects_malformed_rows_and_unsafe_envelope_fields() {
    let body = page_body(1, 1, &payment_row(1, "ordinary"));
    for (body, code) in [
        (
            body.replace(r#","description":"ordinary""#, ""),
            "InvalidResponse",
        ),
        (
            body.replace(r#""paymentAmount":0"#, r#""paymentAmount":-1"#),
            "InvalidResponse",
        ),
        (
            body.replace(r#""number":1"#, r#""number":null"#),
            "InvalidResponse",
        ),
        (
            r#"{"code":"synthetic-peer-canary"}"#.into(),
            "InvalidResponse",
        ),
        (
            r#"{"code":"403","data":["synthetic-peer-canary"]}"#.into(),
            "403",
        ),
    ] {
        let server = Server::start(vec![response("200 OK", body.as_bytes())]);
        let (ctx, _) = RequestContext::new();
        let (error, partial, _) = client(&server, BEARER)
            .list_payments(&ctx, payments())
            .await
            .unwrap_err()
            .into_parts();
        assert_eq!(error.class(), ErrorClass::Api);
        assert_eq!(error.api_error().unwrap().code(), code);
        assert!(!format!("{error:?} {error}").contains("synthetic-peer-canary"));
        assert!(partial.is_none());
    }
}

#[tokio::test]
async fn history_refuses_numeric_tokens_and_credential_key_text() {
    let (ctx, _) = RequestContext::new();
    let row = payment_row(1, "prefix clientseCLIENT_SECRET: synthetic-peer-canary");
    let body = page_body(1, 1, &row);
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let output = client(&server, BEARER)
        .list_payments(&ctx, payments())
        .await
        .unwrap();
    assert!(output.items[0].description.as_deref() == Some("[redacted]"));
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let (error, partial, _) = client(&server, "123456789.0")
        .list_payments(&ctx, payments())
        .await
        .unwrap_err()
        .into_parts();
    assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
    assert!(partial.is_none());
}

#[tokio::test]
async fn history_redacts_superscript_credential_key_text() {
    let (ctx, _) = RequestContext::new();
    let row = payment_row(1, "prefix CLIE²NT_SECRET: synthetic-peer-canary");
    let body = page_body(1, 1, &row);
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let output = client(&server, BEARER)
        .list_payments(&ctx, payments())
        .await
        .unwrap();
    assert!(output.items[0].description.as_deref() == Some("[redacted]"));
}

#[tokio::test]
async fn resources_refuse_conflicting_field_casing_and_credential_reflections() {
    for item in [
        r#"{"artifactId":"ordinary","ArtifactId":"synthetic-conflict"}"#,
        r#"{"statusUI":{"value":"\u0073ynthetic-billing-bearer"}}"#,
        r#"null"#,
    ] {
        let body = format!(r#"{{"code":200,"data":{{"data":[{item}]}}}}"#);
        let server = Server::start(vec![response("200 OK", body.as_bytes())]);
        let (ctx, _) = RequestContext::new();
        let (error, partial, _) = client(&server, BEARER)
            .list_resources(&ctx, ListResourcesInput {})
            .await
            .unwrap_err()
            .into_parts();
        assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
        assert!(partial.is_none());
        assert!(!format!("{error:?} {error}").contains(BEARER));
    }
}

#[tokio::test]
async fn history_and_resources_withhold_rendered_numbers_matching_configured_credentials() {
    let ordinary = payment_row(1, "ordinary");
    let reflected = ordinary.replace(r#""paymentAmount":0"#, r#""paymentAmount":0.0000001"#);
    let server = Server::start(vec![
        response("200 OK", page_body(1, 1, &ordinary).as_bytes()),
        response("200 OK", page_body(1, 1, &reflected).as_bytes()),
        response(
            "200 OK",
            br#"{"code":200,"data":{"data":[{"statusUI":{"value":0.0000001}}]}}"#,
        ),
    ]);
    let config = Config::new(
        ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(Secret::new(BEARER))
            .with_cdn_api_key(Secret::new("1e-7"))
            .with_endpoint_overrides(EndpointOverrides {
                billing: Some(server.origin.clone()),
                ..Default::default()
            }),
    )
    .unwrap();
    config
        .session()
        .set_test_product_root(server.root_der.clone());
    let (ctx, _) = RequestContext::new();
    let billing = Client::new(&config);
    assert!(billing.list_payments(&ctx, payments()).await.is_ok());
    let (error, partial, _) = billing
        .list_payments(&ctx, payments())
        .await
        .unwrap_err()
        .into_parts();
    assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
    assert!(partial.is_none());
    let (error, partial, _) = billing
        .list_resources(&ctx, ListResourcesInput {})
        .await
        .unwrap_err()
        .into_parts();
    assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
    assert!(partial.is_none());
}

#[test]
fn date_range_uses_fixed_zone_and_exclusive_end() {
    use crate::TestClock;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    let now = OffsetDateTime::parse("2024-12-31T17:00:00Z", &Rfc3339).unwrap();
    let clock = TestClock::new(now);
    for (start, end, wire_end) in [
        ("2024-12-31", "", "02/01/2025"),
        ("2024-12-31", "2024-12-31", "01/01/2025"),
        ("2024-02-28", "2024-02-29", "01/03/2024"),
    ] {
        let (_, exclusive) = super::super::history::history_dates(start, end, &clock).unwrap();
        assert_eq!(exclusive, wire_end);
    }
}

#[tokio::test]
async fn history_preserves_baseline_timestamp_syntax() {
    let row = payment_row(1, "ordinary");
    for (timestamp, valid) in [
        ("2024-01-01T1:02:03,12345678901+24:60", true),
        ("2016-12-31T23:59:60Z", false),
    ] {
        let row = row.replace("2024-11-11T18:00:00.123456789+00:00", timestamp);
        let server = Server::start(vec![response("200 OK", page_body(1, 1, &row).as_bytes())]);
        let (ctx, _) = RequestContext::new();
        let result = client(&server, BEARER)
            .list_payments(&ctx, payments())
            .await;
        if valid {
            assert_eq!(result.unwrap().items[0].created_at, timestamp);
        } else {
            assert_eq!(
                result
                    .unwrap_err()
                    .into_parts()
                    .0
                    .api_error()
                    .unwrap()
                    .code(),
                "InvalidResponse"
            );
        }
    }
}

#[tokio::test]
async fn history_cancellation_preserves_identity() {
    let (sent, ready) = tokio::sync::oneshot::channel();
    let server = Server::start(vec![Response::StallBeforeHead(sent)]);
    let (ctx, cancel) = RequestContext::new();
    let client = client(&server, BEARER);
    let cancel_when_started = async {
        ready.await.unwrap();
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(client.list_payments(&ctx, payments()), cancel_when_started);
    let (error, partial, _) = result.unwrap_err().into_parts();
    assert_eq!(error.class(), ErrorClass::Cancelled);
    assert_eq!(error.api_error().unwrap().status(), None);
    assert!(partial.is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn oversized_history_keeps_status_and_refuses_retry() {
    let body = vec![b'x'; (8 << 20) + 1];
    for (status, code, overflow) in [
        ("200 OK", "InvalidResponse", true),
        ("429 Too Many Requests", "Throttled", false),
        ("503 Service Unavailable", "ServerError", false),
    ] {
        let server = Server::start(vec![response(status, &body)]);
        let (ctx, _) = RequestContext::new();
        let (error, partial, _) = client(&server, BEARER)
            .list_payments(&ctx, payments())
            .await
            .unwrap_err()
            .into_parts();
        assert_eq!(error.api_error().unwrap().code(), code);
        assert_eq!(
            error.api_error().unwrap().status(),
            status[..3].parse().ok()
        );
        assert_eq!(error.is_body_too_large(), overflow);
        assert!(partial.is_none());
        assert_eq!(server.requests(), 1);
    }
}

#[tokio::test]
async fn history_walk_counts_only_json_bytes_and_withholds_over_budget() {
    const LIMIT: usize = 32 << 10;
    let row = payment_row(1, "ordinary");
    for extra in [0, 1] {
        let replies = (1..=5)
            .map(|page| {
                let content = row.replace(r#""paymentID":1"#, &format!(r#""paymentID":{page}"#));
                let mut body = page_body(page, 5, &content);
                body.pop();
                body.push_str(r#","padding":""#);
                let size = if page == 5 {
                    (4 << 10) + extra
                } else {
                    7 << 10
                };
                body.push('\0');
                body.extend(std::iter::repeat_n('x', size - body.len() - 2));
                body.push_str("\"}");
                assert!(body.len() < LIMIT);
                let mut bytes = format!(" \n{body}\n ").into_bytes();
                let invalid = bytes.iter_mut().find(|byte| **byte == 0).unwrap();
                *invalid = 0xff;
                response("200 OK", &bytes)
            })
            .collect();
        let server = Server::start(replies);
        let (ctx, _) = RequestContext::new();
        let client = client(&server, BEARER);
        client.session.test_retained_byte_limit.set(LIMIT).unwrap();
        let result = client
            .list_payments(
                &ctx,
                ListPaymentsInput {
                    all: true,
                    ..payments()
                },
            )
            .await;
        if extra == 0 {
            assert_eq!(result.unwrap().items.len(), 5);
        } else {
            let (error, partial, _) = result.unwrap_err().into_parts();
            assert_eq!(error.class(), ErrorClass::HistoryIncomplete);
            assert!(partial.is_none());
        }
        assert_eq!(server.requests(), 5);
    }
}

#[tokio::test]
async fn history_refuses_numeric_tokens_when_request_fails() {
    let body = vec![b'x'; (8 << 20) + 1];
    let server = Server::start(vec![response("503 Service Unavailable", &body)]);
    let (ctx, _) = RequestContext::new();
    let (error, partial, _) = client(&server, "503")
        .list_payments(&ctx, payments())
        .await
        .unwrap_err()
        .into_parts();
    let api = error.api_error().unwrap();
    assert_eq!(api.code(), "InvalidResponse");
    assert!(api.status().is_none());
    assert!(partial.is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn billing_history_exit_refuses_a_reflected_redaction_marker() {
    let body = page_body(1, 1, &payment_row(1, "redacted"));
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let result = client(&server, "redacted")
        .list_payments(&RequestContext::new().0, payments())
        .await;
    assert_eq!(server.requests(), 1);
    assert!(result.is_err());
}

#[tokio::test]
async fn resources_refuse_encoded_credentials_in_unknown_fields() {
    let body = format!(
        r#"{{"code":200,"data":{{"data":[],"unknown":{{"hint":"{}"}}}}}}"#,
        BEARER.replace('-', "%2D")
    );
    let server = Server::start(vec![response("200 OK", body.as_bytes())]);
    let (ctx, _) = RequestContext::new();
    let result = client(&server, BEARER)
        .list_resources(&ctx, ListResourcesInput {})
        .await;
    assert!(result.is_err());
    let failure = result.unwrap_err();
    assert_eq!(
        failure.error().api_error().unwrap().code(),
        "InvalidResponse"
    );
    assert!(failure.partial().is_none());
}
