use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};
use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

const TOKEN: &str = "synthetic-pricing-cli-canary";
const QUOTE: &str =
    include_str!("../../greennode-sdk/tests/fixtures/pricing/GetQuoteServerResizeCredit.json");
const VIP: &str =
    include_str!("../../greennode-sdk/tests/fixtures/pricing/discovery_quote_public_vm.json");
const ESTIMATE: &str =
    r#"{"PublicIPs":[{"Key":"quoted","Kind":"public-vip","Type":"public-vm","Quantity":3}]}"#;
const UNKNOWN: &str = r#"{"PublicIPs":[{"Key":"quoted","Kind":"public-vip","Type":"public-vm","Quantity":3},{"Key":"unknown","Kind":"floating-ip","Quantity":2}]}"#;
const PARTIAL: &str = r#"{"PublicIPs":[{"Key":"unknown","Kind":"floating-ip","Quantity":2},{"Key":"failed","Kind":"public-vip","Type":"public-vm","Quantity":3}]}"#;

struct Source;
impl ConfigSource for Source {
    fn var_os(&self, _name: &OsStr) -> Option<OsString> {
        None
    }
}

fn response(status: &str, raw: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), raw.len().to_string())],
        body: raw.as_bytes().to_vec(),
    }
}

fn invoke(args: &[&str], server: &Server) -> (ExitCode, String, String) {
    invoke_with_bearer(args, server, TOKEN)
}

fn invoke_with_bearer(args: &[&str], server: &Server, bearer: &str) -> (ExitCode, String, String) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let options = with_product_loopback(
        Product::Portal,
        with_product_loopback(
            Product::Storage,
            greennode_sdk::testing::with_clock(
                ConfigOptions::default().with_static_token(Secret::new(bearer)),
                greennode_sdk::testing::retry_clock(),
            ),
            server,
        ),
        server,
    );
    let status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_invocation(Invocation {
            args: [
                "greennode",
                "--region",
                "hcm-3",
                "--project-id",
                "proj-1",
                "--read-only",
                "pricing",
            ]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
            source: &Source,
            config_options: options,
            stdin: &mut std::io::empty(),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context: &context,
        }));
    let (stdout, stderr) = (
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    );
    assert!(!stdout.contains(TOKEN) && !stderr.contains(TOKEN));
    (status, stdout, stderr)
}

fn golden(name: &str, format: &str, suffix: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "tests/fixtures/pricing/{name}.{format}{suffix}.golden"
        )),
    )
    .unwrap()
}

#[test]
fn pricing_reads_match_baseline_in_every_format() {
    for (name, args, status, body, exit) in [
        (
            "get-quote",
            &[
                "get-quote",
                "--action",
                "resize",
                "--resource-type",
                "server",
            ][..],
            "200 OK",
            QUOTE,
            0,
        ),
        (
            "get-quote-refused",
            &["get-quote", "--resource-type", "server"][..],
            "400 Bad Request",
            r#"{"code":"QuoteRefused","message":"quantity is not allowed"}"#,
            1,
        ),
        (
            "estimate",
            &["estimate", "--cli-input-json", ESTIMATE][..],
            "200 OK",
            VIP,
            0,
        ),
        (
            "estimate-unknown",
            &["estimate", "--cli-input-json", UNKNOWN][..],
            "200 OK",
            VIP,
            0,
        ),
        (
            "estimate-partial",
            &["estimate", "--allow-partial", "--cli-input-json", PARTIAL][..],
            "403 Forbidden",
            "{}",
            1,
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(vec![response(status, body)]);
            let args: Vec<_> = ["--output", format]
                .into_iter()
                .chain(args.iter().copied())
                .collect();
            let (status, mut stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::from(exit), "{name}/{format} {stderr}");
            let expected = golden(name, format, "");
            if format == "json" && name.starts_with("estimate") {
                let data: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                let time = data["Metadata"]["QuotedAt"].as_str().unwrap();
                assert!(time.ends_with('Z'));
                stdout = stdout.replacen(time, "2026-01-01T00:00:00Z", 1);
            }
            assert_eq!(stdout, expected, "{name}/{format}");
            assert_eq!(stderr, golden(name, format, ".stderr"), "{name}/{format}");
            assert_eq!(server.requests(), 1);
        }
    }
}

#[test]
fn partial_query_prints_coverage_and_failure_after_reflection() {
    let server = Server::start(vec![response(
        "503 Service Unavailable",
        &format!(r#"{{"reflected":"{TOKEN}"}}"#),
    )]);
    let (status, stdout, stderr) = invoke(
        &[
            "estimate",
            "--allow-partial",
            "--cli-input-json",
            PARTIAL,
            "--query",
            "Items[].Key",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert_eq!(stdout, "[\n  \"unknown\",\n  \"failed\"\n]\n");
    assert!(stderr.contains("Coverage\tprocessing\tfailed"));
    assert!(stderr.contains("DiscoveryIncomplete"));
    assert_eq!(server.requests(), 1);
}

#[test]
fn strict_setup_and_required_quote_field_refuse_before_requests() {
    let server = Server::start(vec![]);
    for json in [
        r#"{"PublicIPs":[{"Key":"one","Key":"two","Kind":"reservation","Quantity":1}]}"#,
        r#"{"PublicIPs":[{"key":"one","Kind":"reservation","Quantity":1}]}"#,
        r#"{"PublicIPs":[["one","reservation","",1]]}"#,
        r#"{"PublicIPs":[{"Key":"one","Kind":"reservation"}]}"#,
        r#"{"PublicIPs":[{"Key":"one","Kind":"reservation","Quantity":1}],"MaxRequests":-1}"#,
    ] {
        let (status, stdout, stderr) = invoke(
            &["estimate", "--cli-input-json", json, "--max-requests", "1"],
            &server,
        );
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(stderr.contains("invalid estimate field"));
    }
    let (status, stdout, stderr) = invoke(&["get-quote"], &server);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(stderr.contains("resource-type"));
    assert_eq!(server.requests(), 0);
}

#[test]
fn pricing_text_and_table_screen_amounts_after_two_decimal_rounding() {
    let bearer = "12345678.24";
    for format in ["table", "text"] {
        let server = Server::start(vec![
            response("200 OK", r#"{"optimumPrice":12345678.239}"#),
            response("200 OK", r#"{"optimumPrice":6172839.1195}"#),
            response("200 OK", r#"{"optimumPrice":12345678.25}"#),
        ]);
        for quantity in [1, 2] {
            let input = format!(
                r#"{{"PublicIPs":[{{"Key":"ip","Kind":"public-vip","Type":"public","Quantity":{quantity}}}]}}"#
            );
            let args = ["estimate", "--output", format, "--cli-input-json", &input];
            let (status, stdout, stderr) = invoke_with_bearer(&args, &server, bearer);
            assert_eq!(status, ExitCode::from(1));
            assert!(!stdout.contains(bearer) && !stderr.contains(bearer));
            assert!(stderr.contains("InvalidResponse"));
        }
        let (status, stdout, _) = invoke_with_bearer(
            &["estimate", "--output", format, "--cli-input-json", ESTIMATE],
            &server,
            bearer,
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stdout.contains("12345678.25"));
        assert_eq!(server.requests(), 3);
    }
}

#[test]
fn authenticated_writer_refuses_reflected_pricing_coverage_diagnostics() {
    let server = Server::start(vec![response("200 OK", VIP)]);
    let (exit, stdout, stderr) = invoke_with_bearer(
        &["estimate", "--output", "text", "--cli-input-json", ESTIMATE],
        &server,
        "Processing",
    );
    assert_eq!(server.requests(), 1);
    assert_eq!(exit, ExitCode::FAILURE);
    assert!(stdout.is_empty());
    assert!(stderr.contains("OutputWithheld"));
}

#[test]
fn estimate_allow_partial_keeps_quotes_on_both_sides_of_a_storage_112() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../greennode-sdk/tests/fixtures/price_catalogue/go-v1.8.0.json"
    ))
    .unwrap();
    let case = cases
        .iter()
        .find(|case| case["service"] == "estimate-partial")
        .unwrap();
    let replies = case["replies"].as_array().unwrap();
    let server = Server::start(
        replies
            .iter()
            .map(|reply| response("200 OK", reply["body"].as_str().unwrap()))
            .collect(),
    );
    let input = r#"{"PublicIPs":[{"Key":"first","Kind":"public-vip","Type":"public","Quantity":1}],"Storage":[{"Key":"refused","Type":"Gold","Quantity":1,"QuotaGB":1},{"Key":"later","Type":"Gold","Quantity":1,"QuotaGB":2}]}"#;
    let (exit, stdout, stderr) = invoke(
        &["estimate", "--allow-partial", "--cli-input-json", input],
        &server,
    );
    assert_eq!(server.requests(), replies.len());
    assert_eq!(exit, ExitCode::FAILURE);
    let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(output["Items"][0]["Price"]["Status"], "quoted");
    assert_eq!(output["Items"][1]["Price"]["Status"], "failed");
    assert_eq!(output["Items"][2]["Price"]["Status"], "quoted");
    assert_eq!(output["Metadata"]["Failures"][0]["Code"], "112");
    let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "DiscoveryIncomplete");
}

#[test]
fn estimate_limit_prints_its_local_diagnostic() {
    let server = Server::start(Vec::new());
    let input = r#"{"PublicIPs":[{"Key":"first","Kind":"public-vip","Type":"public","Quantity":1},{"Key":"second","Kind":"public-vip","Type":"public-vm","Quantity":1}]}"#;
    let (exit, stdout, stderr) = invoke(
        &["estimate", "--max-requests", "1", "--cli-input-json", input],
        &server,
    );
    assert_eq!(server.requests(), 0);
    assert_eq!(exit, ExitCode::FAILURE);
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(
        error,
        serde_json::json!({"error":{
            "code":"EstimateLimit", "message":"estimate limit exceeded: discovery limit exceeded: requires 2 requests, maximum 1"
        }})
    );
}
