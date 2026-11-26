use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "synthetic-cli-portal-bearer";
const USER: &str = include_str!("../../greennode-sdk/tests/fixtures/portal/get_user_info.json");

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "hcm-3".into())
    }
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn invoke(server: &Server, args: &[&str]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_command(server, "get-user-info", args)
}

fn invoke_command(server: &Server, command: &str, args: &[&str]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_billing_loopback(
        ConfigOptions::default()
            .with_static_token(Secret::new(BEARER))
            .with_project_id("proj-1".into())
            .with_endpoint_overrides(EndpointOverrides {
                portal: Some(server.origin.clone()),
                vserver: Some(server.origin.clone()),
                ..Default::default()
            }),
        server,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "portal", command]
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
fn missing_quota_does_not_disclose_name() {
    let server = Server::start(vec![response("200 OK", "[]")]);
    let (status, stdout, stderr) =
        invoke_command(&server, "get-quota", &["--name", "private-canary"]);
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(
        error,
        serde_json::json!({"error": {"code": "NotFound", "message": "greennode: not found: quota"}})
    );
}

#[test]
fn project_reads_match_go_goldens() {
    let cases: &[(&str, &str, [&[u8]; 3])] = &[
        (
            "list-zones",
            include_str!("../../greennode-sdk/tests/fixtures/portal/list_zones.json"),
            [
                include_bytes!("fixtures/portal/list-zones.json.golden"),
                include_bytes!("fixtures/portal/list-zones.table.golden"),
                include_bytes!("fixtures/portal/list-zones.text.golden"),
            ],
        ),
        (
            "list-quota-used",
            include_str!("../../greennode-sdk/tests/fixtures/portal/list_quota_used.json"),
            [
                include_bytes!("fixtures/portal/list-quota-used.json.golden"),
                include_bytes!("fixtures/portal/list-quota-used.table.golden"),
                include_bytes!("fixtures/portal/list-quota-used.text.golden"),
            ],
        ),
        (
            "get-quota",
            include_str!("../../greennode-sdk/tests/fixtures/portal/list_quota_used.json"),
            [
                include_bytes!("fixtures/portal/get-quota.json.golden"),
                include_bytes!("fixtures/portal/get-quota.table.golden"),
                include_bytes!("fixtures/portal/get-quota.text.golden"),
            ],
        ),
        (
            "get-tag-quota",
            include_str!("../../greennode-sdk/tests/fixtures/portal/get_tag_quota.json"),
            [
                include_bytes!("fixtures/portal/get-tag-quota.json.golden"),
                include_bytes!("fixtures/portal/get-tag-quota.table.golden"),
                include_bytes!("fixtures/portal/get-tag-quota.text.golden"),
            ],
        ),
    ];
    for (command, body, expected) in cases {
        let server = Server::start((0..3).map(|_| response("200 OK", body)).collect());
        for (format, expected) in ["json", "table", "text"].into_iter().zip(expected) {
            let mut args = vec!["--output", format];
            if *command == "get-quota" {
                args.extend(["--name", "<name>"]);
            }
            let (status, stdout, stderr) = invoke_command(&server, command, &args);
            assert_eq!(status, ExitCode::SUCCESS, "{command} {format}");
            assert!(stderr.is_empty());
            assert_eq!(&stdout, expected, "{command} {format}");
        }
    }
}

#[test]
fn user_info_matches_go_goldens_in_read_only_mode() {
    let server = Server::start((0..3).map(|_| response("200 OK", USER)).collect());
    for (format, expected) in [
        (
            "json",
            &include_bytes!("fixtures/portal/get-user-info.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("fixtures/portal/get-user-info.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("fixtures/portal/get-user-info.text.golden")[..],
        ),
    ] {
        let (status, stdout, stderr) = invoke(
            &server,
            &["--output", format, "--cli-input-json", "{}", "--read-only"],
        );
        assert_eq!(status, ExitCode::SUCCESS, "{format}");
        assert_eq!(stdout, expected, "{format}");
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
    assert!(
        server
            .heads()
            .iter()
            .all(|head| head.starts_with(b"GET /v1/users/info "))
    );
}

#[test]
fn user_info_redacts_nested_dynamic_keys_before_query() {
    let body = r#"{"data":{"nested":{"apiToken":"private-test-canary","type":"iam-user"}}}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (status, stdout, stderr) = invoke(&server, &["--query", "UserInfo.nested"]);
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(!String::from_utf8_lossy(&stdout).contains("private-test-canary"));
    assert_eq!(
        stdout,
        b"{\n  \"apiToken\": \"\\u003credacted\\u003e\",\n  \"type\": \"iam-user\"\n}\n"
    );
}

#[test]
fn user_info_rejects_invalid_input_before_connecting() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke(&server, &["--cli-input-json", "file://"]);
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(
        error["error"]["message"],
        "--cli-input-json: cannot open file"
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn user_info_reports_permission_error_with_fixed_diagnostic() {
    let server = Server::start(vec![response(
        "403 Forbidden",
        r#"{"code":"PrivateCode","message":"private-test-canary"}"#,
    )]);
    let (status, stdout, stderr) = invoke(&server, &[]);
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(!String::from_utf8_lossy(&stderr).contains("private-test-canary"));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "Forbidden");
    assert_eq!(error["error"]["message"], "portal response withheld");
    assert_eq!(error["error"]["status"], 403);
    assert_eq!(error["error"]["operation"], "portal.GetUserInfo");
}
