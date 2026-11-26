use std::process::ExitCode;

use greennode_sdk::testing::{Response, Server};

use super::{invoke, response, response_with_status};

const RESPONSES: [&str; 5] = [
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/quota_classes.json"),
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/quota_v1_packages.json"),
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/quota_settings.json"),
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/quota_class_packages.json"),
    "[]",
];

macro_rules! goldens {
    ($op:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $op, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".text.golden")),
        ]
    };
}

const FORMATS: [&str; 3] = ["json", "table", "text"];

#[test]
fn synthetic_catalog_matches_go_in_every_format() {
    for (format, golden) in FORMATS.into_iter().zip(goldens!("list-synthetic-packages")) {
        let server = Server::start(RESPONSES.map(response).into());
        let (status, stdout, stderr) =
            invoke(&["list-synthetic-packages", "--output", format], &server);
        assert_eq!(status, ExitCode::SUCCESS, "catalog {format}");
        assert!(stderr.is_empty());
        assert_eq!(String::from_utf8(stdout).unwrap(), golden);
    }
}

#[test]
fn quota_status_renders_partial_rows_in_the_service_view_without_query() {
    let expected_error =
        include_bytes!("../fixtures/monitor/quota-status-partial.json.stderr.golden");
    for (format, golden) in FORMATS.into_iter().zip(goldens!("quota-status-partial")) {
        let server = Server::start(status_responses(true));
        let (status, stdout, stderr) = invoke(
            &[
                "quota-status",
                "--output",
                format,
                "--query",
                "Items[0].Kind",
            ],
            &server,
        );
        assert_eq!(status, ExitCode::from(1));
        assert!(!String::from_utf8_lossy(&stderr).contains("peer-canary"));
        assert_eq!(stderr, expected_error, "quota status partial {format}");
        assert_eq!(snapshot(&stdout, format), golden);
    }
}

#[test]
fn missing_quotas_match_go_in_every_format() {
    for (format, golden) in FORMATS.into_iter().zip(goldens!("quota-status")) {
        let server = Server::start(status_responses(false));
        let (status, stdout, stderr) = invoke(&["quota-status", "--output", format], &server);
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        assert_eq!(snapshot(&stdout, format), golden);
    }
}

#[test]
fn quota_queries_translate_null_after_selection_for_table_and_text_only() {
    for (format, expected) in [
        ("json", "null\n"),
        ("text", "unknown\n"),
        (
            "table",
            "+---------+\n| Value   |\n+---------+\n| unknown |\n+---------+\n",
        ),
    ] {
        let server = Server::start(status_responses(false));
        let (status, stdout, stderr) = invoke(
            &[
                "quota-status",
                "--output",
                format,
                "--query",
                "Items[0].Price",
            ],
            &server,
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        assert_eq!(stdout, expected.as_bytes());
    }
    let server = Server::start(vec![response(include_str!(
        "../../../greennode-sdk/tests/fixtures/monitor/get_dashboard_system.json"
    ))]);
    let (status, stdout, _) = invoke(
        &[
            "get-dashboard",
            "--dashboard-id",
            "dash-1",
            "--output",
            "text",
            "--query",
            "Dashboard.Price",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"\n");
}

fn status_responses(partial: bool) -> Vec<Response> {
    [
        "404 Not Found",
        if partial {
            "403 Forbidden"
        } else {
            "404 Not Found"
        },
        "404 Not Found",
    ]
    .map(|status| response_with_status(status, r#"{"message":"peer-canary"}"#))
    .into()
}

fn snapshot(stdout: &[u8], format: &str) -> String {
    let text = std::str::from_utf8(stdout).unwrap();
    if format == "json" {
        let before = "\"AsOf\": \"";
        let (prefix, rest) = text.split_once(before).unwrap();
        let (_, suffix) = rest.split_once('"').unwrap();
        return format!("{prefix}{before}2000-01-01T00:00:00Z\"{suffix}");
    }
    let (_, suffix) = text
        .strip_prefix("As of ")
        .unwrap()
        .split_once(';')
        .unwrap();
    format!("As of 2000-01-01 00:00:00 UTC;{suffix}")
}

#[test]
fn invalid_quota_kind_reports_go_usage_error_without_a_request() {
    let server = Server::start(Vec::new());
    let (status, stdout, stderr) = invoke(&["quota-status", "--kind", "Slack"], &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"InvalidUsage\",\"message\":\"monitor.GetQuotaStatus: Kind must be synthetic, email, or sms: vngcloud: invalid input\"}}\n");
    assert_eq!(server.requests(), 0);
}

#[test]
fn quota_status_incomplete_names_the_operation_when_cancelled_before_any_request() {
    use greennode::cli::errors::classify;
    use greennode_sdk::monitor::{Client, GetQuotaStatusInput};
    use greennode_sdk::testing::{Product, with_product_loopback};
    use greennode_sdk::{Config, ConfigOptions, RequestContext, Secret};

    let server = Server::start(Vec::new());
    let options = ConfigOptions::default()
        .with_region("hcm-3".into())
        .with_static_token(Secret::new("toy-monitor-token"));
    let client = Client::new(
        &Config::new(with_product_loopback(Product::Monitor, options, &server)).unwrap(),
    );
    let (context, cancel) = RequestContext::new();
    cancel.cancel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let failure = runtime
        .block_on(client.get_quota_status(&context, GetQuotaStatusInput::default()))
        .unwrap_err();
    let registry = greennode::services::registered().unwrap();
    let service = registry
        .services()
        .iter()
        .find(|service| service.name == "monitor")
        .unwrap();
    let classified = classify(failure.error(), service.error_rules);
    assert_eq!(classified.envelope.code, "QuotaStatusIncomplete");
    assert_eq!(
        classified.envelope.operation,
        Some("monitor.GetQuotaStatus")
    );
}
