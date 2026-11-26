use super::*;

const DELETE: &[&str] = &["billing", "delete-budget", "--budget-uuid", "budget-1"];

const CREATE_THRESHOLD: &[&str] = &[
    "billing",
    "create-budget-threshold",
    "--cli-input-json",
    r#"{"BudgetUUID":"budget-1","ThresholdType":"ACTUAL","ThresholdPercentage":100}"#,
];

const UPDATE_THRESHOLD: &[&str] = &[
    "billing",
    "update-budget-threshold",
    "--budget-uuid",
    "budget-1",
    "--threshold-uuid",
    "threshold-1",
];

const DELETE_THRESHOLD: &[&str] = &[
    "billing",
    "delete-budget-threshold",
    "--budget-uuid",
    "budget-1",
    "--threshold-uuid",
    "threshold-1",
];

struct WriteSource {
    env_read_only: bool,
    home: Option<std::path::PathBuf>,
}

impl ConfigSource for WriteSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        match name.to_str()? {
            "GREENNODE_REGION" => Some("hcm-3".into()),
            "VNGCLOUD_READ_ONLY" if self.env_read_only => Some("true".into()),
            _ => None,
        }
    }

    fn home_dir(&self) -> Option<std::path::PathBuf> {
        self.home.clone()
    }
}

const WRITABLE: WriteSource = WriteSource {
    env_read_only: false,
    home: None,
};

fn invoke_write(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_with_source(args, Some(server), BEARER, &WRITABLE)
}

#[test]
fn delete_budget_requires_confirmation_before_connecting() {
    let server = Server::start(vec![]);
    let mut args = DELETE.to_vec();
    args.push("--debug");
    let (status, stdout, stderr) = invoke_write(&args, &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "billing delete-budget is destructive; pass --yes to confirm"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn update_budget_without_changes_reports_the_reason() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke_write(
        &["billing", "update-budget", "--budget-uuid", "budget-1"],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "vngcloud: invalid input: billing.UpdateBudget requires at least one field to change"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn debug_writes_emit_fixed_events_on_success_and_failure() {
    let budget_id = "synthetic-budget-debug-canary";
    for (http, exit) in [("204 No Content", 0), ("403 Forbidden", 1)] {
        let server = Server::start(vec![response(http, String::new())]);
        let (status, stdout, stderr) = invoke_write(
            &[
                "--debug",
                "billing",
                "delete-budget",
                "--budget-uuid",
                budget_id,
                "--yes",
            ],
            &server,
        );
        assert_eq!(status, ExitCode::from(exit));
        assert!(!String::from_utf8_lossy(&stdout).contains(budget_id));
        let stderr = std::str::from_utf8(&stderr).unwrap();
        assert!(!stderr.contains(budget_id));
        let mut lines = stderr.lines();
        assert!(
            lines.next()
                == Some(r#"level=DEBUG msg="write started" operation="billing delete-budget""#)
        );
        assert!(
            lines.next()
                == Some(r#"level=DEBUG msg="write finished" operation="billing delete-budget""#)
        );
        if exit == 0 {
            assert!(lines.next().is_none());
        } else {
            let error: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
            assert_eq!(error["error"]["code"], "Forbidden");
        }
        assert_eq!(server.requests(), 1);
    }
}

#[test]
fn read_only_latches_refuse_writes_before_connecting() {
    let server = Server::start(vec![]);
    let home = std::env::temp_dir().join(format!("greennode-billing-write-{}", std::process::id()));
    let dir = home.join(".greennode");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config"), "[default]\nread_only = true\n").unwrap();
    let env_source = WriteSource {
        env_read_only: true,
        home: None,
    };
    let profile_source = WriteSource {
        env_read_only: false,
        home: Some(home.clone()),
    };

    for (source, latch, extra) in [
        (&WRITABLE, "--read-only", &["--read-only"][..]),
        (
            &env_source,
            "VNGCLOUD_READ_ONLY",
            &["--read-only=false"][..],
        ),
        (&profile_source, "config", &["--read-only=false"][..]),
    ] {
        let mut args = DELETE.to_vec();
        args.extend_from_slice(&["--yes", "--debug"]);
        args.extend_from_slice(extra);
        let (status, stdout, stderr) = invoke_with_source(&args, Some(&server), BEARER, source);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "ReadOnly");
        assert_eq!(
            error["error"]["message"],
            format!("read-only: refused by {latch}")
        );
    }
    std::fs::remove_dir_all(home).unwrap();
    assert_eq!(server.connections(), 0);
}

macro_rules! golden_formats {
    ($command:literal) => {
        [
            (
                "json",
                &include_bytes!(concat!("../fixtures/billing/", $command, ".json.golden"))[..],
            ),
            (
                "table",
                &include_bytes!(concat!("../fixtures/billing/", $command, ".table.golden"))[..],
            ),
            (
                "text",
                &include_bytes!(concat!("../fixtures/billing/", $command, ".text.golden"))[..],
            ),
        ]
    };
}

#[test]
fn writes_match_the_baseline_goldens() {
    for (args, status, body, formats) in [
        (
            &[
                "billing",
                "create-budget",
                "--cli-input-json",
                r#"{"Name":"example-budget","PeriodType":"MONTHLY","Type":"ACTUAL","LimitAmount":1000000}"#,
            ][..],
            "201 Created",
            include_str!("../../../greennode-sdk/src/billing/fixtures/CreateBudget.json"),
            golden_formats!("create-budget"),
        ),
        (
            &[
                "billing",
                "update-budget",
                "--budget-uuid",
                "budget-1",
                "--status",
                "PAUSED",
            ][..],
            "200 OK",
            include_str!("../../../greennode-sdk/src/billing/fixtures/UpdateBudget.json"),
            golden_formats!("update-budget"),
        ),
        (
            &[
                "billing",
                "delete-budget",
                "--budget-uuid",
                "budget-1",
                "--yes",
            ][..],
            "200 OK",
            include_str!("../../../greennode-sdk/src/billing/fixtures/DeleteBudget.json"),
            golden_formats!("delete-budget"),
        ),
        (
            CREATE_THRESHOLD,
            "201 Created",
            include_str!("../../../greennode-sdk/src/billing/fixtures/CreateBudgetThreshold.json"),
            golden_formats!("create-budget-threshold"),
        ),
        (
            &[UPDATE_THRESHOLD, &["--enabled=false"]].concat(),
            "200 OK",
            include_str!("../../../greennode-sdk/src/billing/fixtures/UpdateBudgetThreshold.json"),
            golden_formats!("update-budget-threshold"),
        ),
        (
            &[DELETE_THRESHOLD, &["--yes"]].concat(),
            "200 OK",
            include_str!("../../../greennode-sdk/src/billing/fixtures/DeleteBudgetThreshold.json"),
            golden_formats!("delete-budget-threshold"),
        ),
    ] {
        let server = Server::start(
            formats
                .iter()
                .map(|_| response(status, body.into()))
                .collect(),
        );
        for (format, golden) in formats {
            let mut argv = vec!["--output", format];
            argv.extend_from_slice(args);
            let (exit, stdout, stderr) = invoke_write(&argv, &server);
            assert_eq!(exit, ExitCode::SUCCESS, "{args:?} {format}");
            assert_eq!(stdout, golden, "{args:?} {format}");
            assert!(stderr.is_empty());
        }
        assert_eq!(server.requests(), 3);
    }
}

#[test]
fn write_failures_keep_baseline_exit_codes_and_reconciliation_advice() {
    for (http, body, code, status, exit, advice) in [
        ("503 Service Unavailable", "", "ServerError", 503, 1, true),
        ("401 Unauthorized", "", "Unauthorized", 401, 3, false),
        ("403 Forbidden", "", "Forbidden", 403, 1, false),
        (
            "400 Bad Request",
            include_str!("../../../greennode-sdk/src/billing/fixtures/BudgetNotFound.json"),
            "NotFound",
            400,
            4,
            false,
        ),
    ] {
        let server = Server::start(vec![response(http, body.into())]);
        let mut args = DELETE.to_vec();
        args.extend_from_slice(&["--yes", "--query", "Budget"]);
        let (actual, stdout, stderr) = invoke_write(&args, &server);
        assert_eq!(actual, ExitCode::from(exit));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        assert_eq!(error["error"]["status"], status);
        assert_eq!(error["error"]["operation"], "billing.DeleteBudget");
        assert_eq!(
            error["error"]["message"].as_str().unwrap()
                == "budget write outcome is unconfirmed; read current state before retrying",
            advice,
        );
        assert_eq!(server.requests(), 1);
    }
}

#[test]
fn duplicate_budget_type_keeps_the_server_error_and_exit_one() {
    let server = Server::start(vec![response(
        "400 Bad Request",
        include_str!("../../../greennode-sdk/src/billing/fixtures/DuplicateBudgetType.json").into(),
    )]);
    let (exit, stdout, stderr) = invoke_write(
        &[
            "billing",
            "create-budget",
            "--name",
            "example-budget",
            "--period-type",
            "MONTHLY",
            "--type",
            "ACTUAL",
            "--limit-amount",
            "1000000",
        ],
        &server,
    );
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "BadRequest");
    assert_eq!(error["error"]["status"], 400);
    assert_eq!(
        error["error"]["message"],
        "A budget of type ACTUAL already exists for this user"
    );
    assert_eq!(server.requests(), 1);
}

#[test]
fn delete_threshold_requires_confirmation_before_connecting() {
    let server = Server::start(vec![]);
    let args = [DELETE_THRESHOLD, &["--debug"]].concat();
    let (status, stdout, stderr) = invoke_write(&args, &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "billing delete-budget-threshold is destructive; pass --yes to confirm"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn read_only_refuses_every_threshold_write_before_connecting() {
    let server = Server::start(vec![]);
    for args in [
        CREATE_THRESHOLD.to_vec(),
        [UPDATE_THRESHOLD, &["--enabled=false"]].concat(),
        [DELETE_THRESHOLD, &["--yes"]].concat(),
    ] {
        let args = [&args[..], &["--debug", "--read-only"]].concat();
        let (status, stdout, stderr) = invoke_write(&args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "ReadOnly");
        assert_eq!(
            error["error"]["message"],
            "read-only: refused by --read-only"
        );
        assert!(error["error"]["operation"].is_null());
    }
    assert_eq!(server.connections(), 0);
}

#[test]
fn threshold_update_merges_json_and_flags_and_keeps_false() {
    let server = Server::start(vec![response("204 No Content", String::new())]);
    let args = [
        UPDATE_THRESHOLD,
        &[
            "--cli-input-json",
            r#"{"Enabled":true,"ThresholdPercentage":0,"MaxAlertsPerPeriod":null}"#,
            "--enabled=false",
        ],
    ]
    .concat();
    let (status, stdout, stderr) = invoke_write(&args, &server);
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"{}\n");
    assert!(stderr.is_empty());
    assert_eq!(server.requests(), 1);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&server.bodies()[0]).unwrap()
            == serde_json::json!({"enabled": false, "thresholdPercentage": 0})
    );
}

#[test]
fn invalid_threshold_uuid_names_the_flag_without_echoing_its_value() {
    let server = Server::start(vec![]);
    let value = "invalid_uuid-canary";
    let args = [
        "billing",
        "update-budget-threshold",
        "--budget-uuid",
        "budget-1",
        "--threshold-uuid",
        value,
        "--enabled=false",
    ];
    let (status, stdout, stderr) = invoke_write(&args, &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(!String::from_utf8_lossy(&stderr).contains(value));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "--threshold-uuid must contain only ASCII letters, digits, and hyphens"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn threshold_update_without_changes_reports_the_reason() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke_write(UPDATE_THRESHOLD, &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "greennode: invalid input: billing.UpdateBudgetThreshold requires at least one field to change"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn threshold_delete_emits_fixed_debug_events_on_failure() {
    let server = Server::start(vec![response(
        "503 Service Unavailable",
        format!(r#"{{"code":503,"message":"{BEARER}"}}"#),
    )]);
    let args = [DELETE_THRESHOLD, &["--yes", "--debug"]].concat();
    let (status, stdout, stderr) = invoke_write(&args, &server);
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let stderr = std::str::from_utf8(&stderr).unwrap();
    assert!(!stderr.contains(BEARER));
    assert!(!stderr.contains("threshold-1"));
    let mut lines = stderr.lines();
    assert!(
        lines.next()
            == Some(
                r#"level=DEBUG msg="write started" operation="billing delete-budget-threshold""#
            )
    );
    assert!(
        lines.next()
            == Some(
                r#"level=DEBUG msg="write finished" operation="billing delete-budget-threshold""#
            )
    );
    let error: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "ServerError");
    assert_eq!(error["error"]["status"], 503);
    assert_eq!(error["error"]["operation"], "billing.DeleteBudgetThreshold");
    assert_eq!(
        error["error"]["message"],
        "budget write outcome is unconfirmed; read current state before retrying"
    );
    assert!(lines.next().is_none());
    assert_eq!(server.requests(), 1);
}

#[test]
fn created_threshold_redacts_reflected_strings_before_rendering() {
    let body = format!(
        r#"{{"code":201,"data":{{"uuid":"threshold-1","thresholdType":"ACTUAL","notificationState":"reflected {BEARER}"}}}}"#
    );
    let server = Server::start(
        (0..4)
            .map(|_| response("201 Created", body.clone()))
            .collect(),
    );
    for extra in [
        &["--output", "json"][..],
        &["--output", "table"],
        &["--output", "text"],
        &["--query", "Threshold.NotificationState"],
    ] {
        let args = [CREATE_THRESHOLD, extra].concat();
        let (status, stdout, stderr) = invoke_write(&args, &server);
        assert_eq!(status, ExitCode::SUCCESS);
        let stdout = std::str::from_utf8(&stdout).unwrap();
        assert!(!stdout.contains(BEARER));
        assert!(stdout.contains("redacted"));
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 4);
}
