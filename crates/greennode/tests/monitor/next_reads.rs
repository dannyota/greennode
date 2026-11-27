use std::process::ExitCode;

use greennode_sdk::testing::Server;

use super::{invoke, response_with_status};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../../greennode-sdk/tests/fixtures/monitor/",
            $name,
            ".json"
        ))
    };
}

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
fn notification_quote_and_check_alarm_match_go_formats() {
    for (operation, flags, responses, goldens) in [
        (
            "quote-create-notification-quota",
            &["--kind", "Email", "--package-id", "emailplus"][..],
            &[
                ("404 Not Found", fixture!("notification_quote_absent")),
                ("200 OK", fixture!("notification_quote_packages")),
                ("200 OK", fixture!("notification_quote_paid")),
            ][..],
            goldens!("quote-create-notification-quota"),
        ),
        (
            "get-check-alarm",
            &["--check-id", "chk-1"][..],
            &[
                ("200 OK", fixture!("check_alarm_snapshot")),
                ("200 OK", fixture!("check_alarm_definition")),
            ][..],
            goldens!("get-check-alarm"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(
                responses
                    .iter()
                    .map(|(status, body)| response_with_status(status, body))
                    .collect(),
            );
            let mut args = vec![operation, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(String::from_utf8(stdout).unwrap(), golden);
            assert_eq!(server.requests(), responses.len());
        }
    }
}

#[test]
fn empty_check_alarm_reference_reports_the_go_error_document() {
    let snapshot = fixture!("check_alarm_snapshot").replace("alarm-1", "");
    let server = Server::start(vec![response_with_status("200 OK", &snapshot)]);
    let (status, stdout, stderr) = invoke(&["get-check-alarm", "--check-id", "chk-1"], &server);
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"NotFound\",\"message\":\"check has no alarm reference\",\"operation\":\"monitor.GetCheckAlarm\"}}\n");
    assert_eq!(server.requests(), 1);
}
