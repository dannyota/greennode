use super::{channel_writes::invoke, response_with_status};
use greennode_sdk::testing::Server;
use std::process::ExitCode;

const ALARM: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/log_alarm_guard.json");
const PROJECT: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/remaining_log_project.json");
const PAGE: &str = r#"{"lstData":[],"page":1,"pageSize":10000,"totalItem":0,"totalPage":0}"#;
macro_rules! golden {
    ($op:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $op, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".text.golden")),
        ]
    };
}

#[test]
fn log_alarm_writes_match_go_in_every_output_format() {
    for (operation, args, replies, goldens) in [
        (
            "create-log-alarm",
            &[
                "--name",
                "Read alarm",
                "--log-project-id",
                "project-1",
                "--threshold-value",
                "0",
                "--no-wait",
            ][..],
            &[
                ("200 OK", PAGE),
                ("200 OK", PROJECT),
                ("201 Created", r#"{"data":{"id":"alarm-1"}}"#),
            ][..],
            golden!("create-log-alarm"),
        ),
        (
            "update-log-alarm",
            &[
                "--alarm-id",
                "alarm-1",
                "--description",
                "Changed",
                "--no-wait",
            ][..],
            &[("200 OK", ALARM), ("200 OK", "")][..],
            golden!("update-log-alarm"),
        ),
        (
            "delete-log-alarm",
            &["--alarm-id", "alarm-1", "--yes"][..],
            &[("200 OK", ALARM), ("204 No Content", "")][..],
            golden!("delete-log-alarm"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(
                replies
                    .iter()
                    .map(|(status, body)| response_with_status(status, body))
                    .collect(),
            );
            let mut arguments = vec![operation, "--output", format];
            arguments.extend(args);
            let (status, stdout, stderr) = invoke(&arguments, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty());
            assert!(stdout == golden.as_bytes(), "{operation}/{format}");
            assert_eq!(server.requests(), replies.len());
        }
    }
}

#[test]
fn log_alarm_invalid_threshold_and_destructive_guard_send_nothing() {
    for args in [
        &[
            "create-log-alarm",
            "--name",
            "test",
            "--log-project-id",
            "project-1",
            "--threshold-value",
            "Inf",
        ][..],
        &["delete-log-alarm", "--alarm-id", "alarm-1"][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
        assert_eq!(server.requests(), 0);
    }
}

#[test]
fn channel_flags_keep_commas_and_empty_occurrences() {
    for flag in ["--in-alarm", "--ok"] {
        for value in ["one,two", ""] {
            let server = Server::start(vec![
                response_with_status("200 OK", PAGE),
                response_with_status("200 OK", PROJECT),
                response_with_status("201 Created", "{}"),
            ]);
            let (status, _, _) = invoke(
                &[
                    "create-log-alarm",
                    "--name",
                    "Probe",
                    "--log-project-id",
                    "project-1",
                    "--threshold-value",
                    "0",
                    "--no-wait",
                    flag,
                    value,
                ],
                &server,
            );
            assert_eq!(status, ExitCode::from(2));
            assert_eq!(server.requests(), 0);
        }
    }
}

#[test]
fn resend_codec_refuses_unknown_nested_fields() {
    let server = Server::start(vec![
        response_with_status("200 OK", ALARM),
        response_with_status("200 OK", ""),
    ]);
    let (status, _, _) = invoke(
        &[
            "update-log-alarm",
            "--alarm-id",
            "alarm-1",
            "--no-wait",
            "--cli-input-json",
            r#"{"Resend":{"Enabled":true,"Unknown":1}}"#,
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert_eq!(server.requests(), 0);
}

#[test]
fn superseded_filter_value_is_ignored() {
    let server = Server::start(vec![
        response_with_status("200 OK", ALARM),
        response_with_status("200 OK", ""),
    ]);
    let (status, _, _) = invoke(
        &[
            "update-log-alarm",
            "--alarm-id",
            "alarm-1",
            "--no-wait",
            "--cli-input-json",
            r#"{"Filter":0,"Filter":{},"QueryString":"search"}"#,
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(server.requests(), 2);
}
