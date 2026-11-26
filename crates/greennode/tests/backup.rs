use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BACKENDS: &str = include_str!("fixtures/backup/backends.response.json");
const POLICIES: &str = include_str!("fixtures/backup/policies.response.json");
const POLICY: &str = include_str!("fixtures/backup/get-policy.response.json");
const BEARER: &str = "synthetic-cli-backup-canary";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "han-1".into())
    }
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn invoke(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_billing_loopback(
        ConfigOptions::default()
            .with_static_token(Secret::new(BEARER))
            .with_endpoint_overrides(EndpointOverrides {
                backup_center: Some(format!("{}/vbackup-gateway/", server.origin)),
                ..Default::default()
            }),
        server,
    );
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
    for stream in [&stdout, &stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains(BEARER) && !text.contains("SECRET-MARKER"));
    }
    (status, stdout, stderr)
}

#[test]
fn read_only_backup_output_matches_go_in_each_format() {
    let cases = [
        (
            "get-policy",
            POLICY,
            &["--policy-id", "synthetic"][..],
            [
                include_str!("fixtures/backup/get-policy.json.golden"),
                include_str!("fixtures/backup/get-policy.table.golden"),
                include_str!("fixtures/backup/get-policy.text.golden"),
            ],
        ),
        (
            "list-backends",
            BACKENDS,
            &[][..],
            [
                include_str!("fixtures/backup/list-backends.json.golden"),
                include_str!("fixtures/backup/list-backends.table.golden"),
                include_str!("fixtures/backup/list-backends.text.golden"),
            ],
        ),
        (
            "list-policies",
            POLICIES,
            &[][..],
            [
                include_str!("fixtures/backup/list-policies.json.golden"),
                include_str!("fixtures/backup/list-policies.table.golden"),
                include_str!("fixtures/backup/list-policies.text.golden"),
            ],
        ),
    ];
    for (operation, body, flags, goldens) in cases {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(vec![response("200 OK", body)]);
            let mut args = vec![
                "--read-only",
                "--output",
                format,
                "--project-id",
                "unused",
                "backup",
                operation,
            ];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation} {format}");
            assert!(stderr.is_empty());
            assert_eq!(stdout, golden.as_bytes(), "{operation} {format}");
            assert_eq!(server.requests(), 1);
        }
    }
}

#[test]
fn flags_override_json_and_queries_use_go_keys() {
    let server = Server::start(vec![
        response("200 OK", POLICY),
        response("200 OK", POLICIES),
    ]);
    let (status, stdout, _) = invoke(
        &[
            "backup",
            "get-policy",
            "--cli-input-json",
            r#"{"PolicyID":"wrong"}"#,
            "--policy-id",
            "synthetic",
            "--query",
            "Policy.ID",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"\"synthetic\"\n");
    let (status, stdout, _) = invoke(
        &[
            "backup",
            "list-policies",
            "--cli-input-json",
            r#"{"Page":3,"Size":20}"#,
            "--page",
            "2",
            "--query",
            "Items[0].Config.DailyConfig.Retention",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"7\n");
    let heads = server.heads();
    assert!(heads[0].starts_with(b"GET /vbackup-gateway/v1/backup-policies/synthetic "));
    assert!(heads[1].starts_with(b"GET /vbackup-gateway/v1/backup-policies?page=2&size=20 "));
}

#[test]
fn refusals_exit_before_request() {
    let server = Server::start(vec![]);
    for (args, code, message) in [
        (&["backup", "get-policy"][..], "InvalidUsage", None),
        (
            &["backup", "get-policy", "--policy-id", "../SECRET-MARKER"],
            "InvalidUsage",
            Some(
                "vngcloud: invalid input: backup.GetPolicy requires PolicyID to match ^[A-Za-z0-9-]+$",
            ),
        ),
        (
            &["backup", "list-policies", "--size", "-1"],
            "InvalidUsage",
            Some(
                "vngcloud: invalid input: backup.ListPolicies: Page and Size must not be negative",
            ),
        ),
        (
            &["--region", "hcm-4", "backup", "list-backends"],
            "InvalidConfig",
            Some(
                "vngcloud: invalid config: backup.ListBackends: requires a supported Backup Center region",
            ),
        ),
    ] {
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        if let Some(message) = message {
            assert_eq!(
                error["error"],
                serde_json::json!({"code": code, "message": message})
            );
        }
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn oversized_lists_report_plain_transport_limit_error() {
    let oversized = "x".repeat((8 << 20) + 1);
    let server = Server::start(vec![
        response("200 OK", &oversized),
        response("200 OK", &oversized),
    ]);
    for command in ["list-backends", "list-policies"] {
        let (status, stdout, stderr) = invoke(&["backup", command], &server);
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(
            error["error"],
            serde_json::json!({
                "code": "RequestFailed",
                "message": "transport: response body exceeds limit",
            })
        );
    }
}

#[test]
fn failures_withhold_response_strings_before_rendering() {
    let reflected = BACKENDS.replace("\"backend\"", &format!("\"{BEARER}\""));
    let server = Server::start(vec![
        response("200 OK", &reflected),
        response(
            "404 Not Found",
            r#"{"code":"SECRET-MARKER","message":"SECRET-MARKER"}"#,
        ),
    ]);
    let (status, stdout, stderr) = invoke(
        &[
            "--debug",
            "--output",
            "text",
            "--query",
            "Items[].Name",
            "backup",
            "list-backends",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidResponse");
    assert_eq!(error["error"]["operation"], "backup.ListBackends");

    let (status, stdout, stderr) = invoke(
        &["backup", "get-policy", "--policy-id", "synthetic"],
        &server,
    );
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
    assert_eq!(error["error"]["status"], 404);
}
