use super::{BEARER, invoke, response};
use greennode_sdk::testing::Server;
use std::process::ExitCode;

const TRANSACTION: &str = r#"{"transactionID":1,"transactionType":5,"transactionValue":12.5,"cumulativeValue":100,"creditStatus":false,"sourceReference":"synthetic-reference","userServiceReference":"synthetic-service","invoiceId":null,"description":"synthetic-description","createTimeStamp":"2024-11-11T18:00:00+00:00"}"#;
const PAYMENT: &str = r#"{"paymentID":1,"transactionID":"synthetic-reference","payGate":"use_balance","paymentStatus":"1","paymentAmount":12.5,"paymentType":8084,"description":"synthetic-description","createTimeStamp":"2024-11-11T18:00:00+00:00"}"#;
const RESOURCES: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/ListResources.json");

fn history_body(row: &str) -> String {
    format!(
        r#"{{"code":200,"data":{{"content":[{row}],"pageable":{{"number":1,"size":50,"totalPages":1,"totalElements":1}}}}}}"#
    )
}

#[test]
fn history_lists_match_baseline_goldens() {
    for (command, row, goldens) in [
        (
            "list-transactions",
            TRANSACTION,
            [
                &include_bytes!("../fixtures/render/golden/billing-list-transactions.json.golden")
                    [..],
                &include_bytes!("../fixtures/render/golden/billing-list-transactions.table.golden")
                    [..],
                &include_bytes!("../fixtures/render/golden/billing-list-transactions.text.golden")
                    [..],
            ],
        ),
        (
            "list-payments",
            PAYMENT,
            [
                &include_bytes!("../fixtures/render/golden/billing-list-payments.json.golden")[..],
                &include_bytes!("../fixtures/render/golden/billing-list-payments.table.golden")[..],
                &include_bytes!("../fixtures/render/golden/billing-list-payments.text.golden")[..],
            ],
        ),
    ] {
        let server = Server::start(
            (0..3)
                .map(|_| response("200 OK", history_body(row)))
                .collect(),
        );
        for (format, expected) in ["json", "table", "text"].into_iter().zip(goldens) {
            let (status, stdout, stderr) = invoke(
                &[
                    "billing",
                    command,
                    "--read-only",
                    "--cli-input-json",
                    r#"{"StartDate":"2024-10-01","Page":-1,"All":true}"#,
                    "--all=false",
                    "--start-date",
                    "2024-11-01",
                    "--end-date",
                    "2024-11-30",
                    "--size",
                    "50",
                    "--output",
                    format,
                ],
                Some(&server),
            );
            assert_eq!(status, ExitCode::SUCCESS, "{command} {format}");
            assert!(stdout == expected, "{command} {format} differs from golden");
            assert!(stderr.is_empty());
        }
        let request = format!(
            "GET /gateway/api/v1/{}?dateEnd=01%2F12%2F2024&dateStart=01%2F11%2F2024&pageNumber=1&pageSize=50 ",
            command.trim_start_matches("list-"),
        );
        assert_eq!(server.requests(), 3);
        assert!(
            server
                .heads()
                .iter()
                .all(|head| head.starts_with(request.as_bytes()))
        );
    }
}

#[test]
fn missing_start_date_fails_before_request() {
    let server = Server::start(Vec::new());
    for command in ["list-transactions", "list-payments"] {
        let (status, stdout, stderr) = invoke(&["billing", command], Some(&server));
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("--start-date is required"));
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn history_query_uses_full_output_and_withholds_runtime_failure() {
    let row = PAYMENT.replace(
        "synthetic-description",
        &format!(
            r#"synthetic-description {}\u001b\u009b\u202e"#,
            BEARER.to_uppercase()
        ),
    );
    let server = Server::start(vec![
        response("200 OK", history_body(&row)),
        response("200 OK", history_body(&row)),
    ]);
    let args = [
        "billing",
        "list-payments",
        "--start-date",
        "2024-11-01",
        "--end-date",
        "2024-11-30",
        "--query",
    ];
    let (status, stdout, stderr) = invoke(
        &[args.as_slice(), &["Items[0].Description"]].concat(),
        Some(&server),
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stdout == b"\"synthetic-description [redacted]\\u001b\\u009b\\u202e\"\n");
    assert!(stderr.is_empty());
    let (status, stdout, stderr) = invoke(
        &[args.as_slice(), &["abs(Items[0].Description)"]].concat(),
        Some(&server),
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"QueryFailed\",\"message\":\"billing history query failed; result withheld\"}}\n");
}

#[test]
fn resources_list_matches_baseline_goldens() {
    let server = Server::start(
        (0..3)
            .map(|_| response("200 OK", RESOURCES.into()))
            .collect(),
    );
    for (format, expected) in [
        (
            "json",
            &include_bytes!("../fixtures/billing/list-resources.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("../fixtures/billing/list-resources.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("../fixtures/billing/list-resources.text.golden")[..],
        ),
    ] {
        let (status, stdout, stderr) = invoke(
            &[
                "billing",
                "list-resources",
                "--read-only",
                "--output",
                format,
            ],
            Some(&server),
        );
        assert_eq!(status, ExitCode::SUCCESS, "{format}");
        assert!(stdout == expected, "{format} differs from golden");
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET /gateway/api/v1/resources "))
    );
}

#[test]
fn history_help_explains_dates_and_query_root() {
    let (status, stdout, stderr) = invoke(&["billing", "list-payments", "--help"], None);
    assert_eq!(status, ExitCode::SUCCESS);
    let help = String::from_utf8_lossy(&stdout);
    for text in [
        "--start-date",
        "--end-date",
        "--page",
        "--size",
        "--all",
        "account data",
        "Items",
        "UTC+07:00",
    ] {
        assert!(help.contains(text), "missing help text {text}");
    }
    assert!(!help.contains("--since"));
    assert!(stderr.is_empty());
}

#[test]
fn incomplete_history_walk_fails_without_stdout() {
    let server = Server::start(vec![response(
        "200 OK",
        history_body(PAYMENT)
            .replace(r#""totalPages":1"#, r#""totalPages":2"#)
            .replace(r#""totalElements":1"#, r#""totalElements":51"#),
    )]);
    let (status, stdout, stderr) = invoke(
        &[
            "billing",
            "list-payments",
            "--start-date",
            "2024-11-01",
            "--all",
        ],
        Some(&server),
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "HistoryIncomplete");
    assert_eq!(server.requests(), 1);
}
