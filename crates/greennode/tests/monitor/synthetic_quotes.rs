use std::process::ExitCode;

use greennode_sdk::testing::Server;

use super::{invoke, response, response_with_status};

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
fn both_synthetic_quotes_match_go_in_every_format_and_ignore_order_controls() {
    let cases = [
        (
            "quote-create-synthetic-quota",
            r#"{"ClassID":"class-1","PackageID":"unit_1","Quantity":10,"MaxPrice":-1,"NoWait":true}"#,
            goldens!("quote-create-synthetic-quota"),
            false,
        ),
        (
            "quote-renew-synthetic-quota",
            r#"{"ExpectedEndTime":"2099-02-01T00:00:00Z","MaxPrice":-1,"MaxDaysLeft":-1,"NoWait":true}"#,
            goldens!("quote-renew-synthetic-quota"),
            true,
        ),
    ];
    for (operation, input, goldens, renewal) in cases {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let mut responses = if renewal {
                vec![
                    response(fixture!("synthetic_quote_renewal")),
                    response(fixture!("quota_quota_detail")),
                ]
            } else {
                vec![response_with_status(
                    "404 Not Found",
                    fixture!("synthetic_quote_absent"),
                )]
            };
            responses.extend(
                [
                    fixture!("quota_classes"),
                    fixture!("quota_class_packages"),
                    "[]",
                    fixture!("quota_settings"),
                    fixture!("synthetic_quote_price"),
                ]
                .map(response),
            );
            let count = responses.len();
            let server = Server::start(responses);
            let (status, stdout, stderr) = invoke(
                &[operation, "--cli-input-json", input, "--output", format],
                &server,
            );
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(String::from_utf8(stdout).unwrap(), golden);
            assert_eq!(server.requests(), count);
        }
    }
}

#[test]
fn renewal_guard_errors_have_go_exit_status_and_no_api_metadata() {
    for (reported, expiry, stderr) in [
        (
            fixture!("synthetic_quote_renewal").to_owned(),
            "2099-01-01T00:00:00Z",
            "{\"error\":{\"code\":\"QuotaChanged\",\"message\":\"quota guard changed\"}}\n",
        ),
        (
            fixture!("synthetic_quote_renewal").replace("ACTIVE", "UNKNOWN"),
            "2099-02-01T00:00:00Z",
            "{\"error\":{\"code\":\"UnexpectedStatus\",\"message\":\"monitor.QuoteRenewSyntheticQuota: unknown quota status: monitor: unexpected check status\"}}\n",
        ),
    ] {
        let server = Server::start(vec![
            response(&reported),
            response(fixture!("quota_quota_detail")),
        ]);
        let (status, stdout, errors) = invoke(
            &["quote-renew-synthetic-quota", "--expected-end-time", expiry],
            &server,
        );
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        assert_eq!(errors, stderr.as_bytes());
        assert_eq!(server.requests(), 2);
    }
}
