use super::*;

const DELETE: &[&str] = &["billing", "delete-budget", "--budget-uuid", "budget-1"];

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
    let (status, stdout, stderr) = invoke_write(DELETE, &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(error["error"]["message"], "confirmation requires --yes");
    assert_eq!(server.connections(), 0);
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
        args.push("--yes");
        args.extend_from_slice(extra);
        let (status, stdout, stderr) = invoke_with_source(&args, Some(&server), BEARER, source);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "ReadOnly");
        assert_eq!(
            error["error"]["message"],
            format!("read-only: write refused ({latch})")
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
