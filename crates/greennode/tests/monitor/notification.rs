use std::process::ExitCode;

use greennode_sdk::testing::Server;

use super::{invoke, response, response_with_status};

const QUOTA: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/notification_quota_email.json");
const USAGE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/notification_usage_email.json");

macro_rules! goldens {
    ($name:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $name, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $name, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $name, ".text.golden")),
        ]
    };
}

#[test]
fn notification_reads_match_go_in_every_format() {
    for (operation, body, goldens) in [
        (
            "get-notification-quota",
            QUOTA,
            goldens!("get-notification-quota"),
        ),
        (
            "get-notification-quota-usage",
            USAGE,
            goldens!("get-notification-quota-usage"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(vec![response(body)]);
            let (status, stdout, stderr) =
                invoke(&[operation, "--kind", "Email", "--output", format], &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(String::from_utf8(stdout).unwrap(), golden);
            assert_eq!(server.requests(), 1);
        }
    }
}

#[test]
fn notification_queries_use_utc_display_only_for_table_and_text() {
    for (format, query, expected) in [
        ("json", "Quota.EndTime", "\"2000-02-01T07:00:00+07:00\"\n"),
        (
            "text",
            "Quota.EndTime == '2000-02-01 00:00:00 UTC'",
            "true\n",
        ),
        (
            "table",
            "Quota.EndTime",
            "+-------------------------+\n| Value                   |\n+-------------------------+\n| 2000-02-01 00:00:00 UTC |\n+-------------------------+\n",
        ),
    ] {
        let server = Server::start(vec![response(QUOTA)]);
        let (status, stdout, stderr) = invoke(
            &[
                "get-notification-quota",
                "--kind",
                "Email",
                "--output",
                format,
                "--query",
                query,
            ],
            &server,
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        assert_eq!(String::from_utf8(stdout).unwrap(), expected);
    }
    let dashboard =
        include_str!("../../../greennode-sdk/tests/fixtures/monitor/get_dashboard_system.json")
            .replace("\"updated\"", "\"2000-02-01T07:00:00+07:00\"");
    let server = Server::start(vec![response(&dashboard)]);
    let (status, stdout, stderr) = invoke(
        &[
            "get-dashboard",
            "--dashboard-id",
            "dash-1",
            "--output",
            "text",
            "--query",
            "Dashboard.UpdatedAt",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert_eq!(stdout, b"2000-02-01T07:00:00+07:00\n");
}

#[test]
fn notification_errors_preserve_local_refusal_and_not_found_exit() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) =
        invoke(&["get-notification-quota-usage", "--kind", "SMS"], &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"InvalidUsage\",\"message\":\"monitor.GetNotificationQuotaUsage: SMS usage read is unsupported: vngcloud: invalid input\"}}\n");
    assert_eq!(server.requests(), 0);
    let server = Server::start(vec![response_with_status("404 Not Found", "{}")]);
    let (status, stdout, stderr) = invoke(&["get-notification-quota", "--kind", "Email"], &server);
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"NotFound\",\"message\":\"quota response withheld\",\"status\":404,\"operation\":\"monitor.GetNotificationQuota\"}}\n");
}
