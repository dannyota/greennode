use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const CLUSTERS: &str =
    include_str!("../../greennode-sdk/tests/fixtures/vks/cluster-list-schema.json");
const VERSIONS: &str =
    include_str!("../../greennode-sdk/tests/fixtures/vks/hcm-cluster-versions.json");
const QUOTA: &str = include_str!("../../greennode-sdk/tests/fixtures/vks/hcm-quota.json");
const BEARER: &str = "synthetic-cli-vks-canary";

struct Source<'a>(&'a str);

impl ConfigSource for Source<'_> {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| self.0.into())
    }
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn invoke(region: &str, args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let options = with_billing_loopback(
        ConfigOptions::default()
            .with_static_token(Secret::new(BEARER))
            .with_endpoint_overrides(EndpointOverrides {
                vks: Some(format!("{}/vks-api/", server.origin)),
                ..Default::default()
            }),
        server,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "vks"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source(region),
        config_options: options,
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
fn reads_match_baseline_in_all_formats_under_read_only() {
    for (command, route, body, flags) in [
        (
            "list-clusters",
            "clusters?page=1&pageSize=2",
            CLUSTERS,
            &["--cli-input-json", r#"{"Page":0,"Size":2}"#, "--page", "1"][..],
        ),
        ("list-cluster-versions", "cluster-versions", VERSIONS, &[]),
        ("get-quota", "quota", QUOTA, &[]),
    ] {
        let server = Server::start((0..3).map(|_| response("200 OK", body)).collect());
        for format in ["json", "table", "text"] {
            let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/vks/{command}.{format}.golden"));
            let mut args = vec![
                command,
                "--read-only",
                "--output",
                format,
                "--project-id",
                "unused-project",
            ];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke("hcm-3", &args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(
                stdout,
                std::fs::read(&golden).unwrap(),
                "{command}/{format}"
            );
        }
        assert_eq!(server.requests(), 3);
        assert!(server.heads().iter().all(|head| {
            head.starts_with(format!("GET /vks-api/v1/{route} ").as_bytes())
                && !head
                    .windows(b"unused-project".len())
                    .any(|value| value == b"unused-project")
        }));
    }
}

#[test]
fn query_uses_baseline_keys_and_drops_unknown_response_fields() {
    let body = r#"{"items":[{"id":"cls-1","privateKey":"synthetic-extra-marker","config":{"token":"synthetic-extra-marker"}}],"total":1,"page":0,"pageSize":10}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (status, stdout, stderr) = invoke(
        "han-1",
        &[
            "list-clusters",
            "--query",
            "Items[].{ID:ID,PrivateKey:PrivateKey}",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert_eq!(
        stdout,
        b"[\n  {\n    \"ID\": \"cls-1\",\n    \"PrivateKey\": null\n  }\n]\n"
    );
    assert_eq!(server.requests(), 1);
    assert!(server.heads()[0].starts_with(b"GET /vks-api/v1/clusters?page=0&pageSize=10 "));
}

#[test]
fn invalid_page_and_region_exit_two_without_requests() {
    let server = Server::start(vec![]);
    for (region, args, code, message) in [
        (
            "hcm-3",
            &["list-clusters", "--page", "-1"][..],
            "InvalidUsage",
            "Page must be",
        ),
        (
            "other",
            &["get-quota"][..],
            "InvalidConfig",
            "VKS requires hcm-3 or han-1",
        ),
    ] {
        let (status, stdout, stderr) = invoke(region, args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message)
        );
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn not_found_exits_four_and_withholds_provider_text() {
    let server = Server::start(vec![response(
        "404 Not Found",
        r#"{"code":"synthetic-provider-code","error":{"message":"synthetic-provider-message"}}"#,
    )]);
    let (status, stdout, stderr) = invoke("hcm-3", &["get-quota"], &server);
    assert_eq!(status, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
    assert_eq!(error["error"]["status"], 404);
    assert_eq!(error["error"]["operation"], "vks.GetQuota");
    assert_eq!(
        error["error"]["message"],
        "request failed; response withheld"
    );
    assert_eq!(server.requests(), 1);
}

#[test]
fn help_describes_zero_based_pagination_without_sign_in() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke("other", &["list-clusters", "--help"], &server);
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    let help = String::from_utf8(stdout).unwrap();
    for text in [
        "Pages start at 0",
        "default size of 10",
        "--page 1",
        "--size",
    ] {
        assert!(help.contains(text), "missing {text}");
    }
    assert_eq!(server.connections(), 0);
}
