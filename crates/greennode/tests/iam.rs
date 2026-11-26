#[path = "iam/group_writes.rs"]
mod group_writes;
#[path = "iam/service_account_writes.rs"]
mod service_account_writes;
#[path = "iam/user_writes.rs"]
mod user_writes;

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};
use serde::Deserialize;

const BEARER: &str = "toy/bearer";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "han-1".into())
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/iam")
            .join(name),
    )
    .unwrap()
}

fn response(status: u16, body: Vec<u8>) -> Response {
    Response::Http {
        status: format!("{status} Fixture"),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body,
    }
}

async fn invoke(args: &[&str], server: &Server, bearer: &str) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_product_loopback(
        Product::Iam,
        ConfigOptions::default()
            .with_static_token(Secret::new(bearer))
            .with_endpoint_overrides(EndpointOverrides {
                dashboard: Some(format!("{}/dashboard/", server.origin)),
                iam: Some(format!("{}/iam/", server.origin)),
                ..Default::default()
            }),
        server,
    );
    let status = run_invocation(Invocation {
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
    })
    .await;
    for stream in [&stdout, &stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(
            !text.contains(bearer)
                && !text.contains("private-toy")
                && !text.contains("toy-factor-seed")
                && !text.contains("toy-recovery-code")
        );
    }
    (status, stdout, stderr)
}

#[derive(Deserialize)]
struct Case {
    name: String,
    args: Vec<String>,
    status: u16,
    exit: u8,
    routes: Vec<String>,
}

#[tokio::test]
async fn reads_match_go_routes_exit_codes_and_output() {
    let cases: Vec<Case> = serde_json::from_slice(&fixture("cases.json")).unwrap();
    for case in cases {
        let formats: &[&str] = if case.exit == 0 {
            &["json", "table", "text"]
        } else {
            &["json"]
        };
        for format in formats {
            let attempts = if case.status == 429 { 4 } else { 1 };
            let request_count = attempts * case.routes.len();
            let replies = (0..request_count)
                .map(|index| {
                    let index = index % case.routes.len() + 1;
                    let body = if case.status == 204 {
                        Vec::new()
                    } else {
                        fixture(&format!("{}.{index}.response.json", case.name))
                    };
                    response(case.status, body)
                })
                .collect();
            let server = Server::start(replies);
            let mut args = vec!["--read-only", "--output", format, "iam"];
            args.extend(case.args.iter().map(String::as_str));
            let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
            assert_eq!(
                exit,
                ExitCode::from(case.exit),
                "{} {format}: {}",
                case.name,
                String::from_utf8_lossy(&stderr)
            );
            if case.exit == 0 {
                assert_eq!(
                    String::from_utf8(stdout).unwrap(),
                    String::from_utf8(fixture(&format!("{}.{format}.golden", case.name))).unwrap(),
                    "{} {format}",
                    case.name
                );
                assert!(stderr.is_empty());
            } else {
                assert!(stdout.is_empty());
                assert_eq!(
                    String::from_utf8(stderr).unwrap(),
                    String::from_utf8(fixture(&format!("{}.stderr.golden", case.name))).unwrap(),
                    "{}",
                    case.name
                );
            }
            let heads = server.heads();
            assert_eq!(heads.len(), request_count);
            for (head, route) in heads.iter().zip(case.routes.iter().cycle()) {
                let product = if route.starts_with("/accounts-api/") {
                    "dashboard"
                } else {
                    "iam"
                };
                assert!(head.starts_with(format!("GET /{product}{route} ").as_bytes()));
            }
        }
    }
}

#[tokio::test]
async fn reflection_is_redacted_before_queries_and_rendering() {
    let body = br#"{"id":"user-1","username":"toy%2Fbearer","from":"iam","twoFactorAuth":{"email":{"enabled":false,"secret":"private-toy"}}}"#;
    for format in ["json", "table", "text"] {
        let server = Server::start(vec![response(200, body.to_vec())]);
        let (status, stdout, stderr) = invoke(
            &[
                "--debug",
                "--output",
                format,
                "iam",
                "get-user",
                "--user-id",
                "user-1",
                "--query",
                "User",
            ],
            &server,
            BEARER,
        )
        .await;
        assert_eq!(status, ExitCode::SUCCESS);
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("<redacted>") || output.contains("\\u003credacted\\u003e"));
        assert!(output.contains("user-1"));
        assert!(!output.contains("toy%2Fbearer"));
        assert!(!String::from_utf8_lossy(&stderr).contains("toy%2Fbearer"));
    }
}

#[tokio::test]
async fn required_operands_and_configured_credentials_fail_before_requests() {
    let server = Server::start(vec![]);
    for mut args in [
        vec!["get-user"],
        vec!["get-policy", "--policy-id", "../x"],
        vec!["list-users", "--name", "two words"],
        vec!["get-user", "--user-id", "toy-bearer"],
    ] {
        args.insert(0, "iam");
        let (status, stdout, stderr) = invoke(&args, &server, "toy-bearer").await;
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("InvalidUsage"));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn policy_condition_numbers_use_go_float_output() {
    let body = br#"{"id":"policy-1","statements":[{"condition":{"stringEquals":{"count":1,"limits":[1e6,0.5]}}}]}"#;
    for format in ["json", "table", "text"] {
        let server = Server::start(vec![response(200, body.to_vec())]);
        let (status, stdout, stderr) = invoke(
            &[
                "--output",
                format,
                "iam",
                "get-policy",
                "--policy-id",
                "policy-1",
            ],
            &server,
            BEARER,
        )
        .await;
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let output = String::from_utf8(stdout).unwrap();
        assert!(
            output.contains("1000000") && !output.contains("1.0"),
            "{format}"
        );
        assert!(output.contains(if format == "json" {
            "\"count\": 1,"
        } else {
            "\"count\":1,"
        }));
    }
}

#[tokio::test]
async fn base64_reflections_are_redacted_from_policy_queries_and_errors() {
    const TOKEN: &str = "toy/??!";
    for encoded in ["dG95Lz8/IQ==", "dG95Lz8/IQ", "dG95Lz8_IQ==", "dG95Lz8_IQ"] {
        let body = format!(
            r#"{{"id":"policy-1","name":"{encoded}","statements":[{{"condition":{{"{encoded}":{{"toy":"keep"}},"stringEquals":{{"{encoded}":"keep","kept":"{encoded}","ordinary":"public"}}}}}}]}}"#
        );
        let rejected = format!(r#"{{"message":"value {encoded}"}}"#);
        let server = Server::start(vec![
            response(200, body.into_bytes()),
            response(400, rejected.into_bytes()),
        ]);
        let (status, stdout, stderr) = invoke(
            &[
                "--debug",
                "iam",
                "get-policy",
                "--policy-id",
                "policy-1",
                "--query",
                "Policy.{Name:Name,Condition:Statements[0].Condition}",
            ],
            &server,
            TOKEN,
        )
        .await;
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty() || !String::from_utf8_lossy(&stderr).contains(encoded));
        let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert!(!String::from_utf8_lossy(&stdout).contains(encoded));
        assert!(output["Name"] == "<redacted>");
        assert!(
            output["Condition"]
                == serde_json::json!({"stringEquals":{"kept":"<redacted>","ordinary":"public"}})
        );
        let (status, stdout, stderr) = invoke(
            &["iam", "get-policy", "--policy-id", "policy-1"],
            &server,
            TOKEN,
        )
        .await;
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        assert!(!String::from_utf8_lossy(&stderr).contains(encoded));
    }
}
