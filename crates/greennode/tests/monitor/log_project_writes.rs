use super::{channel_writes::invoke, response_with_status};
use greennode_sdk::testing::Server;
use std::process::ExitCode;

const EMPTY: &str =
    r#"{"content":[],"currentPage":0,"pageSize":100,"totalPages":0,"totalElements":0}"#;
const CLASSES: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/remaining_log_classes.json");
const QUOTE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/monitor/remaining_log_quote.json");
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
fn log_project_order_and_purge_match_go_output() {
    for (op, args, replies, goldens) in [
        (
            "create-log-project",
            &["--name", "Logs", "--max-price", "5", "--no-wait"][..],
            &[
                ("200 OK", EMPTY),
                ("200 OK", CLASSES),
                ("200 OK", QUOTE),
                ("201 Created", r#"{"orderId":4001}"#),
            ][..],
            golden!("create-log-project"),
        ),
        (
            "delete-log-project",
            &[
                "--log-project-id",
                "project-1",
                "--purge",
                "--no-wait",
                "--yes",
            ][..],
            &[("404 Not Found", "{}"), ("204 No Content", "")][..],
            golden!("delete-log-project"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(
                replies
                    .iter()
                    .map(|(status, body)| response_with_status(status, body))
                    .collect(),
            );
            let mut arguments = vec![op, "--output", format];
            arguments.extend(args);
            let (status, stdout, stderr) = invoke(&arguments, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{op}/{format}");
            assert!(stderr.is_empty());
            assert!(stdout == golden.as_bytes(), "{op}/{format}");
            assert_eq!(server.requests(), replies.len());
        }
    }
}
#[test]
fn log_project_blocks_excess_price_and_requires_delete_consent() {
    let server = Server::start(
        [("200 OK", EMPTY), ("200 OK", CLASSES), ("200 OK", QUOTE)]
            .into_iter()
            .map(|(status, body)| response_with_status(status, body))
            .collect(),
    );
    let (status, stdout, stderr) = invoke(
        &["create-log-project", "--name", "Logs", "--no-wait"],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(stderr==b"{\"error\":{\"code\":\"PriceAboveMax\",\"message\":\"vngcloud: quoted price is above MaxPrice: monitor.CreateLogProject: quote 5 VND exceeds MaxPrice 0 VND\"}}\n");
    assert_eq!(server.requests(), 3);
    let server = Server::start(vec![]);
    let (status, stdout, _) = invoke(
        &[
            "delete-log-project",
            "--log-project-id",
            "project-1",
            "--purge",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(server.requests(), 0);
}
