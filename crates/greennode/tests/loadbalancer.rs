#[path = "loadbalancer/create_writes.rs"]
mod create_writes;

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "toy-cli-bearer";

#[tokio::test]
async fn certificate_import_refuses_json_secrets_and_read_only() {
    let server = Server::start(vec![]);
    for (args, expected) in [
        (
            vec![
                "import-certificate",
                "--cli-input-json",
                r#"{"PrivateKey":"toy-key"}"#,
            ],
            "secret field is forbidden",
        ),
        (
            vec![
                "import-certificate",
                "--certificate-file",
                "unused.pem",
                "--private-key-file",
                "-",
                "--read-only",
            ],
            "read-only",
        ),
    ] {
        let (status, stdout, stderr) = invoke(&server, &args, false).await;
        assert_ne!(status, ExitCode::SUCCESS);
        assert!(stdout.is_empty());
        let stderr = String::from_utf8_lossy(&stderr);
        assert!(stderr.contains(expected));
        assert!(!stderr.contains("toy-key"));
    }
}

#[tokio::test]
async fn paid_load_balancer_writes_apply_confirmation_and_read_only() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke(
        &server,
        &["create-load-balancer", "--scheme", "Internet"],
        false,
    )
    .await;
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("pass --yes"));
    for args in [
        vec![
            "create-load-balancer",
            "--scheme",
            "Internal",
            "--read-only",
        ],
        vec!["resize-load-balancer", "--read-only"],
    ] {
        let (status, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("read-only"));
    }
}

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

async fn invoke(server: &Server, args: &[&str], terminal: bool) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let pricing = args[0].starts_with("quote-");
    let product = if pricing {
        Product::Portal
    } else {
        Product::Vlb
    };
    let options = with_product_loopback(
        product,
        ConfigOptions::default()
            .with_static_token(Secret::new(BEARER))
            .with_endpoint_overrides(EndpointOverrides {
                vlb: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/portal/", server.origin)),
                ..Default::default()
            }),
        server,
    );
    invoke_options(args, terminal, options, &mut std::io::empty()).await
}

async fn invoke_options(
    args: &[&str],
    terminal: bool,
    config_options: ConfigOptions,
    stdin: &mut dyn greennode::cli::Stdin,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "loadbalancer"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options,
        stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: terminal,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    })
    .await;
    assert!(!String::from_utf8_lossy(&stdout).contains(BEARER));
    assert!(!String::from_utf8_lossy(&stderr).contains(BEARER));
    (status, stdout, stderr)
}

#[tokio::test]
async fn reads_match_go_goldens() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "list-load-balancers",
            "v2/proj-1/loadBalancers?name=blue+team&page=2&size=5",
            &["--name", "blue team", "--page", "2", "--size", "5"],
        ),
        (
            "get-load-balancer",
            "v2/proj-1/loadBalancers/lb-1",
            &["--load-balancer-id", "lb-1"],
        ),
        (
            "list-packages",
            "v2/proj-1/loadBalancers/packages?zoneId=zone_1",
            &["--zone-id", "zone_1"],
        ),
        (
            "list-certificates",
            "v2/proj-1/cas?name=&page=1&size=10000",
            &[],
        ),
        (
            "get-certificate",
            "v2/proj-1/cas/cert-1",
            &["--certificate-id", "cert-1"],
        ),
        (
            "list-listeners",
            "v2/proj-1/loadBalancers/lb-1/listeners",
            &["--load-balancer-id", "lb-1"],
        ),
        (
            "get-listener",
            "v2/proj-1/loadBalancers/lb-1/listeners/listener-1",
            &["--load-balancer-id", "lb-1", "--listener-id", "listener-1"],
        ),
        (
            "list-pools",
            "v2/proj-1/loadBalancers/lb-1/pools",
            &["--load-balancer-id", "lb-1"],
        ),
        (
            "get-pool",
            "v2/proj-1/loadBalancers/lb-1/pools/pool-1",
            &["--load-balancer-id", "lb-1", "--pool-id", "pool-1"],
        ),
        (
            "get-pool-health-monitor",
            "v2/proj-1/loadBalancers/lb-1/pools/pool-1/healthMonitor",
            &["--load-balancer-id", "lb-1", "--pool-id", "pool-1"],
        ),
        (
            "list-pool-members",
            "v2/proj-1/loadBalancers/lb-1/pools/pool-1/members",
            &["--load-balancer-id", "lb-1", "--pool-id", "pool-1"],
        ),
        (
            "list-policies",
            "v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies",
            &["--load-balancer-id", "lb-1", "--listener-id", "listener-1"],
        ),
        (
            "get-policy",
            "v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies/policy-1",
            &[
                "--load-balancer-id",
                "lb-1",
                "--listener-id",
                "listener-1",
                "--policy-id",
                "policy-1",
            ],
        ),
        (
            "list-tags",
            "v2/proj-1/tag/resource/lb-1",
            &["--load-balancer-id", "lb-1"],
        ),
        (
            "quote-create-load-balancer",
            "portal/v1/price",
            &[
                "--package-id",
                "package-1",
                "--zone-id",
                "zone_1",
                "--scheme",
                "Internal",
                "--subnet-id",
                "subnet-1",
            ],
        ),
        (
            "quote-resize-load-balancer",
            "portal/v1/price",
            &["--load-balancer-id", "lb-1", "--package-id", "package-1"],
        ),
    ];
    for &(command, route, flags) in cases {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let wire = root.join(format!(
            "../greennode-sdk/tests/fixtures/loadbalancer/{command}.json"
        ));
        let body = std::fs::read_to_string(wire).unwrap();
        let server = Server::start((0..3).map(|_| response("200 OK", &body)).collect());
        for format in ["json", "table", "text"] {
            let mut args = vec![
                command,
                "--project-id",
                "proj-1",
                "--read-only",
                "--output",
                format,
            ];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&server, &args, false).await;
            assert_eq!(
                status,
                ExitCode::SUCCESS,
                "{command}/{format}: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(stderr.is_empty());
            let golden = root.join(format!(
                "tests/fixtures/loadbalancer/{command}.{format}.golden"
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
        }
        let method = if command.starts_with("quote-") {
            "POST"
        } else {
            "GET"
        };
        assert_eq!(server.requests(), 3);
        assert!(
            server
                .heads()
                .iter()
                .all(|head| head.starts_with(format!("{method} /{route} ").as_bytes())),
            "{command}"
        );
    }
}

#[tokio::test]
async fn typed_header_is_screened_before_querying() {
    let body = r#"{"data":{"uuid":"listener-1","insertHeaders":[{"headerName":"X-Resource","headerValue":"resource-value"},{"headerName":"X-Own","headerValue":"toy-cli%2Dbearer"}]}}"#;
    let server = Server::start(vec![response("200 OK", body)]);
    let (status, stdout, stderr) = invoke(
        &server,
        &[
            "get-listener",
            "--project-id",
            "proj-1",
            "--load-balancer-id",
            "lb-1",
            "--listener-id",
            "listener-1",
            "--query",
            "Listener.InsertHeaders[].HeaderValue",
            "--output",
            "text",
        ],
        false,
    )
    .await;
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"resource-value\n<redacted>\n");
    assert!(stderr.is_empty());
}

#[tokio::test]
async fn errors_match_go_goldens() {
    for (name, http_status, body, exit) in [
        ("no-data", "200 OK", r#"{"data":null}"#, 1),
        ("bad-json", "200 OK", "{", 1),
        ("bad-shape", "200 OK", r#"{"data":{}}"#, 1),
        (
            "not-found",
            "404 Not Found",
            r#"{"message":"pool not found"}"#,
            4,
        ),
        (
            "forbidden",
            "403 Forbidden",
            r#"{"message":"access denied"}"#,
            1,
        ),
    ] {
        let server = Server::start(vec![response(http_status, body)]);
        let (status, stdout, stderr) = invoke(
            &server,
            &[
                "list-pool-members",
                "--project-id",
                "proj-1",
                "--load-balancer-id",
                "lb-1",
                "--pool-id",
                "pool-1",
            ],
            false,
        )
        .await;
        assert_eq!(status, ExitCode::from(exit), "{name}");
        assert!(stdout.is_empty());
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/loadbalancer/{name}.stderr.golden"));
        assert_eq!(stderr, std::fs::read(golden).unwrap(), "{name}");
    }
}

#[path = "loadbalancer/child_writes.rs"]
mod child_writes;
#[path = "loadbalancer/listener_writes.rs"]
mod listener_writes;
#[path = "loadbalancer/member_writes.rs"]
mod member_writes;
#[path = "loadbalancer/policy_writes.rs"]
mod policy_writes;
#[path = "loadbalancer/pool_writes.rs"]
mod pool_writes;

#[tokio::test]
async fn destructive_delete_requires_yes_and_obeys_read_only() {
    let server = Server::start(vec![]);
    for (command, flag, id) in [
        ("delete-load-balancer", "--load-balancer-id", "lb-1"),
        ("delete-certificate", "--certificate-id", "cert-1"),
    ] {
        let args = [command, flag, id, "--project-id", "proj-1"];
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("--yes"));
        let args = [args.as_slice(), &["--yes", "--read-only"]].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn deletes_match_go_goldens_and_regional_routes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for format in ["json", "table", "text"] {
        for (command, flag, id, route, detail, success) in [
            (
                "delete-load-balancer",
                "--load-balancer-id",
                "lb-1",
                "/v2/proj-1/loadBalancers/lb-1",
                r#"{"data":{"uuid":"lb-1","progressStatus":"CREATED"}}"#,
                "202 Accepted",
            ),
            (
                "delete-certificate",
                "--certificate-id",
                "cert-1",
                "/v2/proj-1/cas/cert-1",
                r#"{"uuid":"cert-1","inUse":false}"#,
                "204 No Content",
            ),
        ] {
            let mut responses = vec![response("200 OK", detail), response(success, "")];
            let mut methods = vec!["GET", "DELETE"];
            if id == "lb-1" {
                responses.push(response("404 Not Found", "{}"));
                methods.push("GET");
            }
            let server = Server::start(responses);
            let (exit, stdout, stderr) = invoke(
                &server,
                &[
                    command,
                    flag,
                    id,
                    "--project-id",
                    "proj-1",
                    "--yes",
                    "--output",
                    format,
                ],
                false,
            )
            .await;
            assert_eq!(exit, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            let golden = root.join(format!(
                "tests/fixtures/loadbalancer/{command}.{format}.golden"
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
            assert_eq!(server.requests(), methods.len());
            for (head, method) in server.heads().iter().zip(methods) {
                assert!(head.starts_with(format!("{method} {route} ").as_bytes()));
            }
        }
    }
}

#[tokio::test]
async fn deletes_report_certificate_in_use_without_api_status() {
    for (responses, message) in [
        (
            vec![response("200 OK", r#"{"uuid":"cert-1","inUse":true}"#)],
            "loadbalancer: certificate in use: loadbalancer.DeleteCertificate: certificate cert-1 is in use",
        ),
        (
            vec![
                response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#),
                response(
                    "409 Conflict",
                    r#"{"message":"certificate is used by a listener"}"#,
                ),
            ],
            "loadbalancer: certificate in use: loadbalancer.DeleteCertificate: certificate cert-1: loadbalancer.DeleteCertificate: certificate is used by a listener (status 409)",
        ),
    ] {
        let requests = responses.len();
        let server = Server::start(responses);
        let (exit, stdout, stderr) = invoke(
            &server,
            &[
                "delete-certificate",
                "--certificate-id",
                "cert-1",
                "--project-id",
                "proj-1",
                "--yes",
            ],
            false,
        )
        .await;
        assert_eq!(exit, ExitCode::from(1));
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "ResourceInUse");
        assert_eq!(envelope["error"]["message"], message);
        assert!(envelope["error"].get("status").is_none());
        assert!(envelope["error"].get("operation").is_none());
        assert_eq!(server.requests(), requests);
    }
}

#[tokio::test]
async fn certificate_refusal_keeps_go_status_based_exits() {
    for (status, message, expected_exit, code) in [
        ("404 Not Found", "certificate is in use", 4, "ResourceInUse"),
        (
            "401 Unauthorized",
            "certificate is in use",
            3,
            "ResourceInUse",
        ),
        ("401 Unauthorized", "login required", 3, "Unauthorized"),
    ] {
        let body = serde_json::json!({"message":message}).to_string();
        let mut responses = vec![
            response("200 OK", r#"{"uuid":"cert-1","inUse":false}"#),
            response(status, &body),
        ];
        if code == "Unauthorized" {
            responses.push(response("400 Bad Request", "{}"));
        }
        let requests = responses.len();
        let server = Server::start(responses);
        let (exit, stdout, stderr) = invoke(
            &server,
            &[
                "delete-certificate",
                "--certificate-id",
                "cert-1",
                "--project-id",
                "proj-1",
                "--yes",
            ],
            false,
        )
        .await;
        assert_eq!(exit, ExitCode::from(expected_exit), "{status}/{code}");
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], code);
        assert_eq!(server.requests(), requests);
    }
}
