use super::*;

const OVERVIEW: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/GetCostOverviewCaptured.json");
const RESOURCES: &str =
    include_str!("../../../greennode-sdk/src/billing/fixtures/ListCostResources.json");
const BALANCES: &str = include_str!("../../../greennode-sdk/src/billing/fixtures/GetBalances.json");

#[test]
fn cost_and_balances_match_baseline_json_table_and_text() {
    let dates = [
        "--cli-input-json",
        r#"{"StartDate":"2026-11-01","EndDate":"2026-11-09"}"#,
    ];

    for (command, body, expected) in [
        (
            "get-cost-overview",
            OVERVIEW,
            [
                &include_bytes!("../fixtures/billing/get-cost-overview.json.golden")[..],
                &include_bytes!("../fixtures/billing/get-cost-overview.table.golden")[..],
                &include_bytes!("../fixtures/billing/get-cost-overview.text.golden")[..],
            ],
        ),
        (
            "list-cost-resources",
            RESOURCES,
            [
                &include_bytes!("../fixtures/billing/list-cost-resources.json.golden")[..],
                &include_bytes!("../fixtures/billing/list-cost-resources.table.golden")[..],
                &include_bytes!("../fixtures/billing/list-cost-resources.text.golden")[..],
            ],
        ),
        (
            "get-balances",
            BALANCES,
            [
                &include_bytes!("../fixtures/billing/get-balances.json.golden")[..],
                &include_bytes!("../fixtures/billing/get-balances.table.golden")[..],
                &include_bytes!("../fixtures/billing/get-balances.text.golden")[..],
            ],
        ),
    ] {
        let server = Server::start((0..3).map(|_| response("200 OK", body.into())).collect());
        for (format, expected) in ["json", "table", "text"].into_iter().zip(expected) {
            let mut args = vec!["billing", command, "--read-only", "--output", format];
            if command != "get-balances" {
                args.extend(dates);
            }
            let (status, stdout, stderr) = invoke(&args, Some(&server));
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(expected),
                "{command}/{format}"
            );
        }
        assert_eq!(server.requests(), 3);
    }
}

#[test]
fn overview_search_flag_overrides_input_json_and_coexists_with_output_query() {
    let server = Server::start(vec![response("200 OK", OVERVIEW.into())]);
    let (status, stdout, stderr) = invoke(
        &[
            "billing",
            "get-cost-overview",
            "--cli-input-json",
            r#"{"StartDate":"2026-11-01","EndDate":"2026-11-09","Query":"json"}"#,
            "--search",
            "flag",
            "--query",
            "GroupedSeries[1].Points[0].Cost",
        ],
        Some(&server),
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"20\n");
    assert!(stderr.is_empty());
    assert!(server.heads()[0].starts_with(b"GET /gateway/api/v2/cost-explorer/overview?endDate=2026-11-09&groupBy=product&interval=daily&q=flag&startDate=2026-11-01 "));
}

#[test]
fn bad_cost_arguments_exit_two_before_requests() {
    let server = Server::start(vec![]);
    for args in [
        &["get-cost-overview", "--start-date", "2026-11-01"][..],
        &[
            "list-cost-resources",
            "--start-date",
            "2026-11-01",
            "--end-date",
            "2026-11-31",
        ],
        &["get-balances", "--cli-input-json", "file://"],
    ] {
        let mut command = vec!["billing"];
        command.extend(args);
        let (status, stdout, stderr) = invoke(&command, Some(&server));
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
    }
    assert_eq!(server.requests(), 0);
}
