use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const FIXTURE: &str = include_str!("../../greennode-sdk/tests/fixtures/project/list_projects.json");
const BEARER: &str = "synthetic-cli-project-canary";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, _: &OsStr) -> Option<OsString> {
        None
    }
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn invoke(args: &[&str], server: Option<&Server>) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let mut config_options = ConfigOptions::default();
    if let Some(server) = server {
        config_options = with_billing_loopback(
            config_options
                .with_endpoint_overrides(EndpointOverrides {
                    vserver: Some(server.origin.clone()),
                    ..Default::default()
                })
                .with_static_token(Secret::new(BEARER)),
            server,
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "project", "list-projects"]
            .into_iter()
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
fn list_projects_uses_configured_filter_and_accepts_a_json_override() {
    let body = r#"{"projects":[{"projectId":"p-hcm","region":"hcm-3"},{"projectId":"p-han","region":"han-1"}]}"#;
    let server = Server::start(vec![response("200 OK", body), response("200 OK", body)]);
    for (input, expected) in [
        ("{}", serde_json::json!(["p-hcm"])),
        (r#"{"Region":"han-1"}"#, serde_json::json!(["p-han"])),
    ] {
        let (status, stdout, stderr) = invoke(
            &["--cli-input-json", input, "--query", "Items[].ID"],
            Some(&server),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&stdout).unwrap(),
            expected
        );
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 2);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET /v1/projects "))
    );
}

#[test]
fn malformed_input_and_query_fail_before_config_and_requests() {
    for args in [
        &["--cli-input-json", r#"{"Region":1}"#][..],
        &["--cli-input-json", "file://"][..],
        &["--query", "["][..],
    ] {
        let (status, stdout, stderr) = invoke(args, None);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "InvalidUsage");
    }
}

#[test]
fn missing_credentials_exit_three() {
    let (status, stdout, stderr) = invoke(&[], None);
    assert_eq!(status, ExitCode::from(3));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
}

#[test]
fn list_projects_matches_baseline_in_every_format_with_read_only_enabled() {
    let server = Server::start((0..3).map(|_| response("200 OK", FIXTURE)).collect());
    for (format, expected) in [
        (
            "json",
            &include_bytes!("fixtures/project/list-projects.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("fixtures/project/list-projects.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("fixtures/project/list-projects.text.golden")[..],
        ),
    ] {
        let (status, stdout, stderr) = invoke(
            &[
                "--output",
                format,
                "--read-only",
                "--project-id",
                "configured-project",
            ],
            Some(&server),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, expected);
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
}

#[test]
fn permission_denial_exits_one_and_withholds_the_response_body() {
    let body = format!(r#"{{"message":"withheld {BEARER}"}}"#);
    let server = Server::start(vec![response("403 Forbidden", &body)]);
    let (status, stdout, stderr) = invoke(&[], Some(&server));
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "Forbidden");
    assert_eq!(error["error"]["status"], 403);
    assert_eq!(error["error"]["operation"], "project.ListProjects");
    assert!(!String::from_utf8_lossy(&stderr).contains("withheld"));
    assert_eq!(server.requests(), 1);
}
