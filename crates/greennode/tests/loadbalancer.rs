use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "toy-cli-bearer";

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
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let status = run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "loadbalancer"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_product_loopback(
            product,
            ConfigOptions::default()
                .with_static_token(Secret::new(BEARER))
                .with_endpoint_overrides(EndpointOverrides {
                    vlb: Some(format!("{}/", server.origin)),
                    portal: Some(format!("{}/portal/", server.origin)),
                    ..Default::default()
                }),
            server,
        ),
        stdin: &mut stdin,
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
