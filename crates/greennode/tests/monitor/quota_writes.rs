use super::{channel_writes::invoke, response_with_status};
use greennode_sdk::testing::Server;
use std::process::ExitCode;
macro_rules! fixture {
    ($file:literal) => {
        include_str!(concat!(
            "../../../greennode-sdk/tests/fixtures/monitor/",
            $file,
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
const EMAIL: &str = fixture!("notification_quota_email");
const SYNTH: &str = fixture!("synthetic_quote_renewal");
const DETAIL: &str = fixture!("quota_quota_detail");
const ABSENT: &str = fixture!("notification_quote_absent");
const SYNTH_ABSENT: &str = fixture!("synthetic_quote_absent");
const PACKAGES: &str = fixture!("notification_quote_packages");
const FREE: &str = r#"{"optimumPrice":0}"#;
const WARNING: &str =
    include_str!("../fixtures/monitor/set-synthetic-quota-auto-renew.stderr.golden");

#[test]
fn quota_lifecycle_writes_match_go_output_and_warning() {
    let email_enabled = EMAIL.replace("\"autoRenew\": 0", "\"autoRenew\": 1");
    let email_disabled = EMAIL.replace("\"autoRenewPeriod\": 1", "\"autoRenewPeriod\": 0");
    let synth_enabled = SYNTH.replace("\"autoRenew\": 0", "\"autoRenew\": 1");
    for (op, flags, replies, goldens, warn) in [
        (
            "create-notification-quota",
            &["--kind", "Email", "--no-wait"][..],
            &[
                ("404 Not Found", ABSENT),
                ("200 OK", PACKAGES),
                ("200 OK", FREE),
                ("404 Not Found", ABSENT),
                ("200 OK", r#"{"orderId":"order-1","paymentUrl":null}"#),
            ][..],
            golden!("create-notification-quota"),
            "",
        ),
        (
            "delete-notification-quota",
            &[
                "--kind",
                "Email",
                "--yes",
                "--expected-quota-id",
                "quota-email",
            ][..],
            &[
                ("200 OK", EMAIL),
                ("200 OK", EMAIL),
                ("204 No Content", ""),
                ("404 Not Found", ABSENT),
            ][..],
            golden!("delete-notification-quota"),
            "",
        ),
        (
            "set-notification-quota-auto-renew",
            &["--kind", "Email", "--enabled=false"][..],
            &[
                ("200 OK", email_enabled.as_str()),
                ("200 OK", email_enabled.as_str()),
                ("200 OK", ""),
                ("200 OK", email_disabled.as_str()),
            ][..],
            golden!("set-notification-quota-auto-renew"),
            "",
        ),
        (
            "delete-synthetic-quota",
            &["--expected-quota-id", "quota-1", "--yes"][..],
            &[
                ("200 OK", SYNTH),
                ("200 OK", DETAIL),
                ("200 OK", SYNTH),
                ("200 OK", DETAIL),
                ("204 No Content", ""),
                ("404 Not Found", SYNTH_ABSENT),
            ][..],
            golden!("delete-synthetic-quota"),
            "",
        ),
        (
            "set-synthetic-quota-auto-renew",
            &["--enabled=true"][..],
            &[
                ("200 OK", SYNTH),
                ("200 OK", DETAIL),
                ("200 OK", FREE),
                ("200 OK", SYNTH),
                ("200 OK", DETAIL),
                ("200 OK", ""),
                ("200 OK", synth_enabled.as_str()),
                ("200 OK", DETAIL),
            ][..],
            golden!("set-synthetic-quota-auto-renew"),
            WARNING,
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(
                replies
                    .iter()
                    .map(|(status, body)| response_with_status(status, body))
                    .collect(),
            );
            let mut args = vec![op, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{op}/{format}");
            assert_eq!(stderr, warn.as_bytes(), "{op}/{format}");
            assert_eq!(stdout, golden.as_bytes(), "{op}/{format}");
            assert_eq!(server.requests(), replies.len());
        }
    }
}

#[test]
fn warning_is_opt_in_and_runs_only_after_the_guards() {
    let server = Server::start(vec![]);
    let (status, _, stderr) = invoke(
        &[
            "create-notification-quota",
            "--kind",
            "Email",
            "--max-price",
            "1",
            "--read-only",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert!(!String::from_utf8_lossy(&stderr).contains("Warning:"));
    assert_eq!(server.requests(), 0);
    let server = Server::start(vec![response_with_status("403 Forbidden", "{}")]);
    let (status, _, stderr) = invoke(
        &[
            "create-notification-quota",
            "--kind",
            "Email",
            "--max-price",
            "1",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stderr.starts_with(b"Warning: pay: true may debit the balance"));
    assert_eq!(server.requests(), 1);
    let server = Server::start(vec![response_with_status("403 Forbidden", "{}")]);
    let (status, stdout, stderr) = invoke(
        &["delete-log-alarm", "--alarm-id", "alarm-1", "--yes"],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(!String::from_utf8_lossy(&stderr).contains("Warning:"));
}

#[test]
fn synthetic_delete_effect_survives_both_parse_and_preflight_refusals() {
    for args in [
        &[
            "delete-synthetic-quota",
            "--expected-quota-id",
            "quota-1",
            "--yes",
            "--query=",
        ][..],
        &[
            "delete-synthetic-quota",
            "--expected-quota-id",
            "quota-1",
            "--yes",
            "--purge=invalid",
        ][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(!stderr.is_empty());
        let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(
            json["Effect"],
            "Deleting this quota permanently deletes every synthetic check."
        );
        assert_eq!(json["Stage"], "");
        assert_eq!(server.requests(), 0);
    }
}

#[test]
fn quota_warnings_follow_their_go_dispatch_phases() {
    let server = Server::start(vec![]);
    let (status, _, stderr) = invoke(
        &[
            "ensure-synthetic-quota",
            "--max-price",
            "1",
            "--read-only",
            "--debug",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert!(stderr.starts_with(b"Warning: pay: true may debit the balance"));
    assert_eq!(server.requests(), 0);
    for args in [
        &[
            "create-notification-quota",
            "--kind",
            "Email",
            "--max-price",
            "1",
            "--debug",
        ][..],
        &[
            "set-synthetic-quota-auto-renew",
            "--enabled=true",
            "--debug",
        ][..],
    ] {
        let server = Server::start(vec![response_with_status("403 Forbidden", "{}")]);
        let (_, _, stderr) = invoke(args, &server);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.find("write started").unwrap() < stderr.find("Warning:").unwrap());
    }
}

#[test]
fn quota_refusals_match_go_error_envelopes() {
    let cases = [
        (
            &["delete-notification-quota", "--kind", "Email", "--yes"][..],
            vec![
                response_with_status("200 OK", EMAIL),
                response_with_status("200 OK", EMAIL),
                response_with_status("409 Conflict", r#"{"message":"ignored"}"#),
            ],
            &include_bytes!("../fixtures/monitor/notification-quota-delete-refused.stderr.golden")
                [..],
        ),
        (
            &["create-notification-quota", "--kind", "Email"][..],
            vec![
                response_with_status("404 Not Found", ABSENT),
                response_with_status("200 OK", PACKAGES),
                response_with_status("200 OK", r#"{"optimumPrice":1}"#),
            ],
            &include_bytes!("../fixtures/monitor/notification-quota-price-refused.stderr.golden")[..],
        ),
    ];
    for (args, replies, golden) in cases {
        let count = replies.len();
        let server = Server::start(replies);
        let (status, _, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(1));
        assert!(stderr == golden);
        assert_eq!(server.requests(), count);
    }
}
