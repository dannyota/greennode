use std::process::ExitCode;

use super::{channel_writes::invoke, response_with_status};
use greennode_sdk::monitor::QuotaTimestamp;
use greennode_sdk::testing::{Response, Server};
use serde_json::Value;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../../greennode-sdk/tests/fixtures/monitor/",
            $name,
            ".json"
        ))
    };
}
macro_rules! golden {
    ($op:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $op, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".text.golden")),
        ]
    };
}

const ACTIVE: &str = fixture!("synthetic_quote_renewal");
const DETAIL: &str = fixture!("quota_quota_detail");
const ABSENT: &str = fixture!("synthetic_quote_absent");
const CREATE: &[&str] = &[
    "--class-id",
    "class-1",
    "--package-id",
    "unit_1",
    "--quantity",
    "10",
];
const RENEW: &[&str] = &["--expected-end-time", "2099-02-01T00:00:00Z"];
const RECOVER: &[&str] = &["--expected-quota-id", "quota-1"];
const ENSURE: &[&str] = &["--expected-quota-id", "quota-1", "--max-days-left", "0"];

fn catalog() -> impl Iterator<Item = Response> {
    [
        fixture!("quota_classes"),
        fixture!("quota_class_packages"),
        "[]",
        fixture!("quota_settings"),
    ]
    .into_iter()
    .map(|body| response_with_status("200 OK", body))
}

fn responses(quota: Option<&str>, status: &str, reply: &str) -> Vec<Response> {
    let read = || match quota {
        Some(quota) => vec![
            response_with_status("200 OK", quota),
            response_with_status("200 OK", DETAIL),
        ],
        None => vec![response_with_status("404 Not Found", ABSENT)],
    };
    read()
        .into_iter()
        .chain(catalog())
        .chain([response_with_status("200 OK", r#"{"optimumPrice":0}"#)])
        .chain(catalog())
        .chain(read())
        .chain([response_with_status(status, reply)])
        .collect()
}

fn expected_create(golden: &str, stdout: &str, format: &str) -> String {
    const FIXED: &str = "2000-01-01T00:00:00Z";
    let stamp = match format {
        "table" => stdout
            .lines()
            .nth(3)
            .unwrap()
            .split('|')
            .nth(7)
            .unwrap()
            .trim()
            .to_owned(),
        "text" => stdout.trim_end().split('\t').nth(6).unwrap().to_owned(),
        _ => serde_json::from_str::<Value>(stdout).unwrap()["SentAt"]
            .as_str()
            .unwrap()
            .to_owned(),
    };
    assert!(serde_json::from_value::<QuotaTimestamp>(Value::String(stamp.clone())).is_ok());
    let expected = golden.replace(FIXED, &stamp);
    if format != "table" {
        return expected;
    }
    expected
        .replace(
            "SentAt               |",
            &format!("{:<width$} |", "SentAt", width = stamp.len()),
        )
        .replace(
            "----------------------+",
            &format!("{}+", "-".repeat(stamp.len() + 2)),
        )
}

#[test]
fn synthetic_order_writes_match_go_output_in_all_formats() {
    let trash = ACTIVE
        .replace("ACTIVE", "IN-TRASH")
        .replace("2099-", "2000-");
    for (op, flags, quota, goldens) in [
        (
            "create-synthetic-quota",
            CREATE,
            None,
            golden!("create-synthetic-quota"),
        ),
        (
            "renew-synthetic-quota",
            RENEW,
            Some(ACTIVE),
            golden!("renew-synthetic-quota"),
        ),
        (
            "recover-synthetic-quota",
            RECOVER,
            Some(trash.as_str()),
            golden!("recover-synthetic-quota"),
        ),
        (
            "ensure-synthetic-quota",
            ENSURE,
            Some(ACTIVE),
            golden!("ensure-synthetic-quota"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let replies = responses(quota, "200 OK", r#"{"orderId":"synth-order"}"#);
            let count = replies.len();
            let server = Server::start(replies);
            let mut args = vec![op, "--output", format, "--max-price", "0", "--no-wait"];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{op}/{format}");
            assert!(stderr.is_empty(), "{op}/{format}");
            let stdout = String::from_utf8(stdout).unwrap();
            let expected = if quota.is_none() {
                expected_create(golden, &stdout, format)
            } else {
                golden.into()
            };
            assert_eq!(stdout, expected, "{op}/{format}");
            assert_eq!(server.requests(), count);
        }
    }
}

const MESSAGE: &str =
    "quota command failed; inspect the outcome and reconcile with reads before another write";

fn fixed_error(code: &str, operation: &str, status: u16) -> String {
    format!(
        "{{\"error\":{{\"code\":\"{code}\",\"message\":\"{MESSAGE}\",\"operation\":\"monitor.{operation}\",\"status\":{status}}}}}\n"
    )
}

#[test]
fn synthetic_preflight_declines_parse_errors_query_and_missing_or_null_consent() {
    for (op, operation, flags) in [
        ("create-synthetic-quota", "CreateSyntheticQuota", CREATE),
        ("renew-synthetic-quota", "RenewSyntheticQuota", RENEW),
        ("recover-synthetic-quota", "RecoverSyntheticQuota", RECOVER),
        ("ensure-synthetic-quota", "EnsureSyntheticQuota", ENSURE),
    ] {
        for extra in [
            &[][..],
            &["--cli-input-json", r#"{"MaxPrice":null}"#],
            &["--read-only"],
            &["--max-price", "0", "--query="],
            &["--max-price", "nonnumeric"],
        ] {
            let server = Server::start(vec![]);
            let mut args = vec![op];
            args.extend(flags);
            args.extend(extra);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::from(2));
            let outcome: Value = serde_json::from_slice(&stdout).unwrap();
            assert_eq!(outcome["Sent"], "no");
            assert_eq!(outcome["Stage"], "preflight");
            assert_eq!(outcome["Error"]["Code"], "InvalidUsage");
            assert_eq!(stderr, fixed_error("InvalidUsage", operation, 0).as_bytes());
            assert_eq!(server.requests(), 0);
        }
    }
}

#[test]
fn ensure_uses_the_default_window_and_accepts_explicit_json_zero_price() {
    let replies = [
        response_with_status("200 OK", ACTIVE),
        response_with_status("200 OK", DETAIL),
    ]
    .into_iter()
    .chain(catalog())
    .collect();
    let server = Server::start(replies);
    let (status, stdout, stderr) = invoke(
        &[
            "ensure-synthetic-quota",
            "--cli-input-json",
            r#"{"MaxPrice":0}"#,
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    let outcome: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(outcome["Action"], "none");
    assert_eq!(outcome["Sent"], "no");
    assert_eq!(outcome["Stage"], "not-due");
    assert_eq!(server.requests(), 6);
}

#[test]
fn paid_order_refusal_keeps_the_status_in_stdout_and_fixed_stderr() {
    let replies = responses(
        Some(ACTIVE),
        "409 Conflict",
        r#"{"message":"renewal refused"}"#,
    );
    let server = Server::start(replies);
    let (status, stdout, stderr) = invoke(
        &[
            "renew-synthetic-quota",
            "--expected-end-time",
            "2099-02-01T00:00:00Z",
            "--max-price",
            "0",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    let outcome: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(outcome["Stage"], "refused");
    assert_eq!(outcome["Sent"], "yes");
    assert_eq!(outcome["Error"]["Status"], 409);
    assert_eq!(
        stderr,
        fixed_error(
            outcome["Error"]["Code"].as_str().unwrap(),
            "RenewSyntheticQuota",
            409
        )
        .as_bytes()
    );
    assert_eq!(server.requests(), 14);
}

#[test]
fn ensure_confirms_renewal_only_after_readback() {
    let extended = ACTIVE.replace("2099-02-01", "2099-03-01");
    let mut replies = responses(Some(ACTIVE), "200 OK", r#"{"orderId":"synth-order"}"#);
    replies.extend([
        response_with_status("200 OK", &extended),
        response_with_status("200 OK", DETAIL),
    ]);
    let server = Server::start(replies);
    let (status, stdout, stderr) = invoke(
        &[
            "ensure-synthetic-quota",
            "--max-days-left",
            "0",
            "--max-price",
            "0",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(
        stdout
            == include_bytes!("../fixtures/monitor/ensure-synthetic-quota-confirmed.json.golden")
    );
    assert_eq!(server.requests(), 16);
}
