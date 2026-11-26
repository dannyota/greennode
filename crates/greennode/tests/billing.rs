#[path = "billing/budget_reads.rs"]
mod budget_reads;
#[path = "billing/cost.rs"]
mod cost;

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};

const BUDGET: &str = include_str!("fixtures/billing/budget-response.json");
const BEARER: &str = "synthetic-cli-billing-canary";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        match name.to_str()? {
            "GREENNODE_REGION" => Some("hcm-3".into()),
            "VNGCLOUD_READ_ONLY" => Some("true".into()),
            _ => None,
        }
    }
}

fn response(status: &str, body: String) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.into_bytes(),
    }
}

fn invoke(args: &[&str], server: Option<&Server>) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let mut config_options = ConfigOptions::default();
    if let Some(server) = server {
        config_options = with_billing_loopback(
            config_options.with_static_token(Secret::new(BEARER)),
            server,
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: std::iter::once("greennode")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    assert!(!String::from_utf8_lossy(&stdout).contains(BEARER));
    assert!(!String::from_utf8_lossy(&stderr).contains(BEARER));
    (status, stdout, stderr)
}

#[test]
fn list_budgets_matches_json_table_and_text() {
    let server = Server::start(
        (0..3)
            .map(|_| response("200 OK", format!(r#"{{"code":200,"data":[{BUDGET}]}}"#)))
            .collect(),
    );
    for (format, expected) in [
        (
            "json",
            &include_bytes!("fixtures/billing/list-budgets.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("fixtures/billing/list-budgets.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("fixtures/billing/list-budgets.text.golden")[..],
        ),
    ] {
        let (status, stdout, stderr) = invoke(
            &[
                "--output",
                format,
                "billing",
                "list-budgets",
                "--read-only",
                "--cli-input-json",
                r#"{"Status":"PAUSED"}"#,
                "--status",
                "ACTIVE",
            ],
            Some(&server),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, expected);
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
    assert!(server.heads().iter().all(|head| {
        head.starts_with(b"GET /gateway/api/v1/budgets?status=ACTIVE&view=summary ")
    }));
}

#[test]
fn get_budget_matches_json_table_and_text() {
    let server = Server::start(
        (0..3)
            .map(|_| response("200 OK", format!(r#"{{"code":200,"data":{BUDGET}}}"#)))
            .collect(),
    );
    for (format, expected) in [
        (
            "json",
            &include_bytes!("fixtures/billing/get-budget.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("fixtures/billing/get-budget.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("fixtures/billing/get-budget.text.golden")[..],
        ),
    ] {
        let (status, stdout, stderr) = invoke(
            &[
                "billing",
                "get-budget",
                "--cli-input-json",
                r#"{"BudgetUUID":"b-1"}"#,
                "--output",
                format,
            ],
            Some(&server),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, expected);
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| { head.starts_with(b"GET /gateway/api/v1/budgets/b-1 ") })
    );
}

#[test]
fn query_reads_the_baseline_keys() {
    let server = Server::start(vec![response(
        "200 OK",
        format!(r#"{{"code":200,"data":{BUDGET}}}"#),
    )]);
    let (status, stdout, stderr) = invoke(
        &[
            "billing",
            "get-budget",
            "--budget-uuid",
            "b-1",
            "--query",
            "Budget.UUID",
        ],
        Some(&server),
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"\"b-1\"\n");
    assert!(stderr.is_empty());
}

#[test]
fn not_found_exits_four_with_safe_api_metadata() {
    let server = Server::start(vec![response(
        "404 Not Found",
        format!(r#"{{"code":"Missing","message":"rejected {BEARER}","data":null}}"#),
    )]);
    let (status, stdout, stderr) = invoke(
        &["billing", "get-budget", "--budget-uuid", "b-1"],
        Some(&server),
    );
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
    assert_eq!(error["error"]["status"], 404);
    assert_eq!(error["error"]["operation"], "billing.GetBudget");
}

#[test]
fn invalid_query_wins_over_missing_credentials() {
    let (status, stdout, stderr) = invoke(&["billing", "list-budgets", "--query", "["], None);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["message"], "invalid query");
}

#[test]
fn missing_credentials_exits_three() {
    let (status, stdout, stderr) = invoke(&["billing", "list-budgets"], None);
    assert_eq!(status, ExitCode::from(3));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
}

#[test]
fn non_null_cost_matches_the_go_json_golden() {
    let server = Server::start(vec![response(
        "200 OK",
        include_str!("fixtures/billing/get-budget-cost.response.json").into(),
    )]);
    let (status, stdout, stderr) = invoke(
        &["billing", "get-budget", "--budget-uuid", "b-1"],
        Some(&server),
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(
        stdout,
        include_bytes!("fixtures/billing/get-budget-cost.json.golden")
    );
    assert!(stderr.is_empty());
}

#[path = "billing/history.rs"]
mod history;
