use super::{invoke, response};
use greennode_sdk::testing::Server;
use std::process::ExitCode;

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
    ($name:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $name, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $name, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $name, ".text.golden")),
        ]
    };
}
#[test]
fn invalid_history_range_exits_two_without_request() {
    let server = Server::start(vec![]);
    let (status, _, _) = invoke(
        &[
            "list-check-alarm-history",
            "--check-id",
            "chk-1",
            "--start",
            "2000-01-01 00:00:00Z",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert_eq!(server.requests(), 0);
}

#[test]
fn synthetic_usage_descriptions_keep_typed_values() {
    let usage = fixture!("remaining_synthetic_usage").replace(
        "\"api\": {\n      \"name\"",
        "\"authorization\": {\n      \"name\"",
    );
    let server = Server::start(vec![response(&usage)]);
    let (status, stdout, _) = invoke(&["get-synthetic-quota-usage", "--output", "json"], &server);
    assert_eq!(status, ExitCode::SUCCESS);
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(
        value["Usage"]["Descriptions"]["authorization"]["Name"],
        "API"
    );
}

#[test]
fn check_request_maps_keep_typed_values() {
    let check = fixture!("remaining_check")
        .replace("\"X-Value\": \"one\"", "\"Authorization\": \"safe-value\"");
    let server = Server::start(vec![response(&check)]);
    let (status, stdout, _) = invoke(
        &["get-check", "--check-id", "chk-1", "--output", "json"],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(
        value["Check"]["Config"]["Request"]["Headers"]["Authorization"],
        "safe-value"
    );
}

#[test]
fn remaining_reads_match_go() {
    let mut mismatches = Vec::new();
    for (operation, flags, bodies, goldens) in [
        (
            "list-check-alarm-history",
            &[
                "--check-id",
                "chk-1",
                "--start",
                "2000-01-01T07:00:00+07:00",
                "--end",
                "2000-01-05T00:00:00Z",
            ][..],
            &[
                fixture!("check_alarm_snapshot"),
                fixture!("check_alarm_definition"),
                fixture!("remaining_history"),
            ][..],
            golden!("list-check-alarm-history"),
        ),
        (
            "get-alarm",
            &["--alarm-id", "alarm-1"][..],
            &[fixture!("remaining_alarm")][..],
            golden!("get-alarm"),
        ),
        (
            "list-alarms",
            &["--kind", "Log"][..],
            &[fixture!("remaining_alarms")][..],
            golden!("list-alarms"),
        ),
        (
            "get-channel",
            &["--channel-id", "channel-1"][..],
            &[fixture!("remaining_channels")][..],
            golden!("get-channel"),
        ),
        (
            "list-channels",
            &[][..],
            &[fixture!("remaining_channels")][..],
            golden!("list-channels"),
        ),
        (
            "list-channel-types",
            &[][..],
            &[fixture!("remaining_channel_types")][..],
            golden!("list-channel-types"),
        ),
        (
            "get-check",
            &["--check-id", "chk-1"][..],
            &[fixture!("remaining_check")][..],
            golden!("get-check"),
        ),
        (
            "list-checks",
            &[][..],
            &[fixture!("remaining_checks")][..],
            golden!("list-checks"),
        ),
        (
            "list-locations",
            &[][..],
            &[fixture!("remaining_locations")][..],
            golden!("list-locations"),
        ),
        (
            "get-log-project",
            &["--log-project-id", "project-1"][..],
            &[fixture!("remaining_log_project")][..],
            golden!("get-log-project"),
        ),
        (
            "list-log-projects",
            &[][..],
            &[fixture!("remaining_log_projects")][..],
            golden!("list-log-projects"),
        ),
        (
            "list-log-project-classes",
            &[][..],
            &[fixture!("remaining_log_classes")][..],
            golden!("list-log-project-classes"),
        ),
        (
            "quote-create-log-project",
            &["--name", "Logs"][..],
            &[
                fixture!("remaining_log_classes"),
                fixture!("remaining_log_quote"),
            ][..],
            golden!("quote-create-log-project"),
        ),
        (
            "get-quota-usage",
            &[][..],
            &[fixture!("remaining_quota_usage")][..],
            golden!("get-quota-usage"),
        ),
        (
            "get-synthetic-quota",
            &[][..],
            &[fixture!("quota_synthetic"), fixture!("quota_quota_detail")][..],
            golden!("get-synthetic-quota"),
        ),
        (
            "get-synthetic-quota-usage",
            &[][..],
            &[fixture!("remaining_synthetic_usage")][..],
            golden!("get-synthetic-quota-usage"),
        ),
        (
            "list-notification-packages",
            &["--kind", "Email"][..],
            &[fixture!("notification_quote_packages")][..],
            golden!("list-notification-packages"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(bodies.iter().map(|body| response(body)).collect());
            let mut args = vec![operation, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty(), "{operation}/{format}");
            let actual = String::from_utf8(stdout).unwrap();
            if actual != golden {
                mismatches.push((operation, format, actual, golden));
            }
            assert_eq!(server.requests(), bodies.len());
            let routes: Vec<_> = server
                .heads()
                .iter()
                .map(|head| {
                    let head = std::str::from_utf8(head).unwrap();
                    let mut segments = head.split_whitespace();
                    let method = segments.next().unwrap();
                    let path = segments.next().unwrap().split('?').next().unwrap();
                    format!("{method} {path}")
                })
                .collect();
            assert_eq!(routes, routes_for(operation));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

fn routes_for(operation: &str) -> &'static [&'static str] {
    match operation {
        "list-check-alarm-history" => &[
            "GET /vmonitor-uptime-manager/v1/uptimes/chk-1",
            "GET /vmonitor-api/api/v1/alarms/synthetic/metrics/mona/alarm-1",
            "GET /vmonitor-api/api/v1/alarms/synthetic/metrics/mona/sub-1/histories",
        ],
        "get-alarm" => &["GET /vmonitor-api/api/v1/alarms/alarm-1"],
        "list-alarms" => &["GET /vmonitor-api/api/v1/alarms/list"],
        "get-channel" => &["GET /notification-gateway/api/v1/notification/list/typeSearch"],
        "list-channels" => &["GET /notification-gateway/api/v1/notification/list/typeSearch"],
        "list-channel-types" => &["GET /notification-gateway/api/v1/type/list"],
        "get-check" => &["GET /vmonitor-uptime-manager/v1/uptimes/chk-1"],
        "list-checks" => &["GET /vmonitor-uptime-manager/v1/uptimes"],
        "list-locations" => &["GET /vmonitor-uptime-manager/v1/locations"],
        "get-log-project" => &["GET /log-api/v1/projects/project-1"],
        "list-log-projects" => &["GET /log-api/v1/projects"],
        "list-log-project-classes" => &["GET /billing-api/v2/log/quota-class"],
        "quote-create-log-project" => &[
            "GET /billing-api/v2/log/quota-class",
            "POST /billing-api/v2/log/prices/created-price",
        ],
        "get-quota-usage" => &["GET /billing-api/v1/quotas/usages"],
        "get-synthetic-quota" => &[
            "GET /billing-api/v1/synthetic/quota",
            "GET /billing-api/v2/synthetic/quota-1/quota-detail",
        ],
        "get-synthetic-quota-usage" => &["GET /billing-api/v1/synthetic/quota/usages"],
        "list-notification-packages" => &["GET /billing-api/v1/email/packages"],
        _ => panic!("unknown Monitor command"),
    }
}
