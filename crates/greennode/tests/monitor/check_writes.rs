use super::{channel_writes::invoke, response_with_status};
use greennode_sdk::testing::Server;
use std::process::ExitCode;

const CHECK: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/remaining_check.json");
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
fn five_check_writes_match_go_in_all_formats() {
    let changed = CHECK.replace("\"Probe\"", "\"Changed\"");
    let disabled = CHECK.replace("ENABLED", "DISABLED");
    for (op, flags, bodies, goldens) in [
        (
            "create-check",
            &[
                "--name",
                "Probe",
                "--url",
                "https://example.test/",
                "--locations",
                "loc-1",
            ][..],
            &[("201 Created", CHECK)][..],
            golden!("create-check"),
        ),
        (
            "update-check",
            &["--check-id", "chk-1", "--name", "Changed"][..],
            &[("200 OK", CHECK), ("200 OK", changed.as_str())][..],
            golden!("update-check"),
        ),
        (
            "delete-check",
            &["--check-id", "chk-1", "--yes"][..],
            &[("204 No Content", "")][..],
            golden!("delete-check"),
        ),
        (
            "pause-check",
            &["--check-id", "chk-1"][..],
            &[
                ("200 OK", CHECK),
                ("204 No Content", ""),
                ("200 OK", disabled.as_str()),
            ][..],
            golden!("pause-check"),
        ),
        (
            "resume-check",
            &["--check-id", "chk-1"][..],
            &[
                ("200 OK", disabled.as_str()),
                ("204 No Content", ""),
                ("200 OK", CHECK),
            ][..],
            golden!("resume-check"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(
                bodies
                    .iter()
                    .map(|(status, body)| response_with_status(status, body))
                    .collect(),
            );
            let mut args = vec![op, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{op}/{format}");
            assert!(stderr.is_empty());
            assert!(stdout == golden.as_bytes(), "{op}/{format}");
            assert_eq!(server.requests(), bodies.len());
        }
    }
}

#[test]
fn notification_flags_are_unsplit_but_locations_use_comma_separated_values() {
    let server = Server::start(vec![response_with_status("201 Created", CHECK)]);
    let (status, _, _) = invoke(
        &[
            "create-check",
            "--name",
            "Probe",
            "--url",
            "https://example.test/",
            "--locations",
            "loc-1,loc-2",
            "--in-alarm-channel-id",
            "one,two",
            "--in-alarm-channel-id",
            "three",
            "--cli-input-json",
            r#"{"Notifications":{"Up":["old"]}}"#,
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    let body: serde_json::Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
    assert!(body["locations"] == serde_json::json!(["loc-1", "loc-2"]));
    assert!(
        body["notifications"]
            == serde_json::json!({"In-alarm":["one,two","three"],"Up":["old"],"Undetermined":[]})
    );
    let server = Server::start(vec![]);
    let (status, _, _) = invoke(
        &[
            "update-check",
            "--check-id",
            "chk-1",
            "--in-alarm-channel-id",
            "one,two",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert_eq!(server.requests(), 0);
    let (status, stdout, stderr) = invoke(&["create-check", "--help"], &server);
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("--in-alarm-channel-id <stringArray>")
    );
}

#[test]
fn check_write_guards_run_before_the_network() {
    for args in [
        &["pause-check", "--check-id", "chk-1", "--read-only"][..],
        &["delete-check", "--check-id", "chk-1"][..],
        &[
            "update-check",
            "--check-id",
            "chk-1",
            "--cli-input-json",
            r#"{"Notifications":{"InAlarm":["x"]}}"#,
        ][..],
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
fn check_outputs_keep_probe_headers_as_typed_request_fields() {
    let response = CHECK.replace(
        "\"X-Value\": \"one\"",
        "\"Authorization\": \"Bearer probe-canary\"",
    );
    let server = Server::start(vec![response_with_status("201 Created", &response)]);
    let (status, stdout, stderr) = invoke(
        &[
            "create-check",
            "--name",
            "Probe",
            "--url",
            "https://example.test/",
            "--locations",
            "loc-1",
            "--query",
            "Check.Config.Request.Headers.Authorization",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(stdout == b"\"Bearer probe-canary\"\n");
}

#[test]
fn generated_check_reference_keeps_string_array_and_slice_type_names() {
    let docs = include_str!("../../../../docs/wiki/CLI-Monitor.md");
    let create = docs
        .split("## create-check\n")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap();
    assert!(
        create
            .lines()
            .any(|line| line.contains("`--in-alarm-channel-id`") && line.contains("`stringArray`"))
    );
    assert!(
        create
            .lines()
            .any(|line| line.contains("`--locations`") && line.contains("`[]string`"))
    );
}
