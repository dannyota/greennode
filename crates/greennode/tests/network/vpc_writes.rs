use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::*;

const FIXTURES: &str = "tests/fixtures/network";

#[derive(Deserialize)]
struct Case {
    command: String,
    args: Vec<String>,
    requests: Vec<Request>,
}

#[derive(Deserialize)]
struct Request {
    method: String,
    path: String,
    status: u16,
    response: Option<Value>,
    body: Option<Value>,
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("../fixtures/network/vpc-writes.json")).unwrap()
}

fn run(
    server: &Server,
    command: &str,
    args: &[&str],
    format: &str,
    read_only: bool,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let options = with_product_loopback(
        Product::Vserver,
        ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
        server,
    );
    invoke_in_mode(options, "proj-1", format, command, args, read_only)
}

#[test]
fn network_writes_match_go_commands_requests_and_goldens() {
    for case in cases() {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                case.requests
                    .iter()
                    .map(|request| {
                        let body = request
                            .response
                            .as_ref()
                            .map_or_else(String::new, Value::to_string);
                        response(&format!("{} Test", request.status), &body)
                    })
                    .collect(),
            );
            let args: Vec<_> = case.args.iter().map(String::as_str).collect();
            let (exit, stdout, stderr) = run(&server, &case.command, &args, format, false);
            assert_eq!(
                exit,
                ExitCode::SUCCESS,
                "{} ({format}) {}",
                case.command,
                String::from_utf8_lossy(&stderr)
            );
            assert!(stderr.is_empty());
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(FIXTURES)
                .join(format!("{}.{format}.golden", case.command));
            assert!(
                stdout == std::fs::read(path).unwrap(),
                "{} ({format}) output differs",
                case.command
            );
            assert_eq!(server.requests(), case.requests.len(), "{}", case.command);
            let heads = server.heads();
            let bodies = server.bodies();
            for ((head, body), request) in heads.iter().zip(&bodies).zip(&case.requests) {
                let want = format!("{} /v2/proj-1/{} ", request.method, request.path);
                assert!(
                    head.starts_with(want.as_bytes()),
                    "{} used the wrong route",
                    case.command
                );
                match &request.body {
                    Some(want) => assert!(
                        serde_json::from_slice::<Value>(body)
                            .as_ref()
                            .is_ok_and(|value| value == want),
                        "{} sent the wrong body",
                        case.command
                    ),
                    None => assert!(body.is_empty(), "{} sent an unexpected body", case.command),
                }
            }
        }
    }
}

#[test]
fn every_network_write_refuses_read_only_mode_before_requests() {
    let server = Server::start(Vec::new());
    for case in cases() {
        let args: Vec<_> = case.args.iter().map(String::as_str).collect();
        let (exit, stdout, stderr) = run(&server, &case.command, &args, "json", true);
        assert_ne!(exit, ExitCode::SUCCESS, "{}", case.command);
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn network_guarded_commands_require_yes_before_requests() {
    let server = Server::start(Vec::new());
    for case in cases()
        .into_iter()
        .filter(|case| case.args.iter().any(|arg| arg == "--yes"))
    {
        let args: Vec<_> = case
            .args
            .iter()
            .map(String::as_str)
            .filter(|arg| *arg != "--yes")
            .collect();
        let (exit, stdout, stderr) = run(&server, &case.command, &args, "json", false);
        assert_eq!(exit, ExitCode::from(2), "{}", case.command);
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("pass --yes to confirm"));
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn remove_acl_rule_distinguishes_missing_priority_from_priority_zero() {
    let server = Server::start(Vec::new());
    let (exit, stdout, stderr) = run(
        &server,
        "remove-network-acl-rule",
        &[
            "--network-acl-id",
            "acl-1",
            "--direction",
            "inbound",
            "--yes",
        ],
        "json",
        false,
    );
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("--priority is required"));
    assert_eq!(server.requests(), 0);
}

#[test]
fn create_dhcp_options_distinguishes_empty_dns_servers_from_missing() {
    let server = Server::start(Vec::new());
    for (json, expected) in [
        (r#"{}"#, "--dns-servers is required"),
        (r#"{"DNSServers":null}"#, "--dns-servers is required"),
        (
            r#"{"DNSServers":[]}"#,
            "vngcloud: invalid input: network.CreateDHCPOptions requires at least one DNS server",
        ),
    ] {
        let (exit, stdout, stderr) = run(
            &server,
            "create-dhcp-options",
            &["--name", "test", "--cli-input-json", json],
            "json",
            false,
        );
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["message"], expected);
    }
    assert_eq!(server.requests(), 0);
}
