use super::*;

const COST: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/GetCurrentPeriodCost.json");
const THRESHOLDS: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/ListBudgetThresholds.json");
const ALERTS: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/ListBudgetAlerts.json");

#[test]
fn budget_reads_reject_numeric_bearer_reflection() {
    for (args, body, number) in [
        (&["get-current-period-cost"][..], COST, "4200000"),
        (
            &["list-budget-thresholds", "--budget-uuid", "budget-1"],
            THRESHOLDS,
            "100.0",
        ),
        (
            &["list-budget-alerts", "--budget-uuid", "budget-1"],
            ALERTS,
            "4200000",
        ),
    ] {
        assert_numeric_bearer_rejected(args, body, number);
    }
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
fn current_period_cost_matches_json_table_and_text() {
    assert_formats(
        &["get-current-period-cost", "--cli-input-json", "{}"],
        COST,
        b"GET /gateway/api/v1/budgets/cost/overview ",
        golden_formats!("get-current-period-cost"),
    );
}

#[test]
fn budget_thresholds_matches_json_table_and_text() {
    assert_formats(
        &[
            "list-budget-thresholds",
            "--cli-input-json",
            r#"{"BudgetUUID":"budget-1"}"#,
        ],
        THRESHOLDS,
        b"GET /gateway/api/v1/budgets/budget-1/thresholds ",
        golden_formats!("list-budget-thresholds"),
    );
}

#[test]
fn budget_alerts_matches_json_table_and_text() {
    assert_formats(
        &[
            "list-budget-alerts",
            "--cli-input-json",
            r#"{"BudgetUUID":"unused","PeriodKey":"unused"}"#,
            "--budget-uuid",
            "budget-1",
            "--period-key",
            "2026-09",
        ],
        ALERTS,
        b"GET /gateway/api/v1/budgets/budget-1/alerts?periodKey=2026-09 ",
        golden_formats!("list-budget-alerts"),
    );
}

fn assert_formats(args: &[&str], body: &str, path: &[u8], formats: [(&str, &[u8]); 3]) {
    let server = Server::start(
        formats
            .iter()
            .map(|_| response("200 OK", body.into()))
            .collect(),
    );
    for (format, expected) in formats {
        let mut argv = vec!["--output", format, "--read-only", "billing"];
        argv.extend_from_slice(args);
        let (status, stdout, stderr) = invoke(&argv, Some(&server));
        assert_eq!(status, ExitCode::SUCCESS, "{args:?} {format}");
        assert_eq!(stdout, expected, "{args:?} {format}");
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), formats.len());
    assert!(server.heads().iter().all(|head| head.starts_with(path)));
}

#[test]
fn budget_read_queries_use_baseline_keys() {
    for (body, args, query, expected) in [
        (
            COST,
            &["get-current-period-cost"][..],
            "PeriodCost.ActualCost",
            &b"4200000\n"[..],
        ),
        (
            THRESHOLDS,
            &["list-budget-thresholds", "--budget-uuid", "budget-1"][..],
            "Items[0].UUID",
            &b"\"threshold-1\"\n"[..],
        ),
        (
            ALERTS,
            &["list-budget-alerts", "--budget-uuid", "budget-1"][..],
            "Items[0].Recipients[0]",
            &b"\"\\u003caccount\\u003e\"\n"[..],
        ),
    ] {
        let server = Server::start(vec![response("200 OK", body.into())]);
        let mut argv = vec!["billing", "--query", query];
        argv.extend_from_slice(args);
        let (status, stdout, stderr) = invoke(&argv, Some(&server));
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, expected);
        assert!(stderr.is_empty());
    }
}

#[test]
fn budget_reads_refuse_invalid_input_without_connecting() {
    let server = Server::start(vec![]);
    for (command, args, message) in [
        (
            "get-current-period-cost",
            &["--cli-input-json", "file://"][..],
            "--cli-input-json: cannot open file",
        ),
        (
            "list-budget-thresholds",
            &[][..],
            "--budget-uuid is required",
        ),
        (
            "list-budget-alerts",
            &["--budget-uuid", "a/b"][..],
            "request failed",
        ),
    ] {
        let mut argv = vec!["billing", command];
        argv.extend_from_slice(args);
        let (status, stdout, stderr) = invoke(&argv, Some(&server));
        assert_eq!(status, ExitCode::from(2), "{command}");
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "InvalidUsage");
        assert_eq!(error["error"]["message"], message);
    }
    assert_eq!(server.connections(), 0);
}

#[test]
fn budget_lists_do_not_classify_using_a_reflected_message() {
    for command in ["list-budget-thresholds", "list-budget-alerts"] {
        let server = Server::start(vec![response(
            "400 Bad Request",
            format!(r#"{{"code":400,"message":"Budget not found: {BEARER}","data":null}}"#),
        )]);
        let (status, stdout, stderr) = invoke(
            &["billing", command, "--budget-uuid", "budget-1"],
            Some(&server),
        );
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "BadRequest");
        assert_eq!(error["error"]["status"], 400);
    }
}

#[test]
fn current_period_cost_reports_forbidden() {
    let server = Server::start(vec![response(
        "403 Forbidden",
        r#"{"code":403,"message":"Forbidden","data":null}"#.into(),
    )]);
    let (status, stdout, stderr) = invoke(&["billing", "get-current-period-cost"], Some(&server));
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "Forbidden");
    assert_eq!(error["error"]["status"], 403);
    assert_eq!(error["error"]["operation"], "billing.GetCurrentPeriodCost");
}
