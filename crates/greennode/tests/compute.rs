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
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let status = run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "compute"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_product_loopback(
            Product::Vserver,
            ConfigOptions::default()
                .with_static_token(Secret::new(BEARER))
                .with_endpoint_overrides(EndpointOverrides {
                    vserver: Some(format!("{}/", server.origin)),
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
    for (command, route, flags) in [
        (
            "get-server",
            "v2/proj-1/servers/server-1",
            &["--server-id", "server-1"][..],
        ),
        (
            "get-server-console-log",
            "v2/proj-1/servers/server-1/console-log",
            &["--server-id", "server-1"],
        ),
        (
            "get-server-group",
            "v2/proj-1/serverGroups/group-1",
            &["--server-group-id", "group-1"],
        ),
        (
            "get-ssh-key",
            "v2/proj-1/sshKeys/key-1",
            &["--ssh-key-id", "key-1"],
        ),
        (
            "list-flavor-zones",
            "v1/proj-1/flavor_zones/product",
            &["--zone-id", "zone-1"],
        ),
        (
            "list-flavors",
            "v1/proj-1/general-1/flavors",
            &["--flavor-zone-id", "general-1"],
        ),
        ("list-gpu-images", "v1/proj-1/images/gpu", &[]),
        (
            "list-os-images",
            "v1/proj-1/images/os?zoneId=zone-1",
            &["--zone-id", "zone-1"],
        ),
        (
            "list-server-group-members",
            "v2/proj-1/serverGroups?limit=10000&name=&offset=0",
            &[],
        ),
        (
            "list-server-group-policies",
            "v2/proj-1/serverGroups/policies",
            &[],
        ),
        (
            "list-server-groups",
            "v2/proj-1/serverGroups?limit=10000&name=&offset=0",
            &[],
        ),
        (
            "list-server-security-groups",
            "v2/proj-1/servers?page=1&size=10000",
            &[],
        ),
        ("list-servers", "v2/proj-1/servers?page=1&size=10000", &[]),
        (
            "list-ssh-keys",
            "v2/proj-1/sshKeys?name=&page=1&size=10000",
            &[],
        ),
        (
            "list-user-images",
            "v2/proj-1/user-images?page=1&size=10000",
            &[],
        ),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let wire = root.join(format!(
            "../greennode-sdk/tests/fixtures/compute/{command}.json"
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
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            let golden = root.join(format!("tests/fixtures/compute/{command}.{format}.golden"));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
        }
        assert_eq!(server.requests(), 3);
        assert!(
            server
                .heads()
                .iter()
                .all(|head| head.starts_with(format!("GET /{route} ").as_bytes()))
        );
    }
}

#[tokio::test]
async fn console_text_escapes_terminal_controls_and_withholds_query_errors() {
    let raw = r#"{"data":"boot\nready\t\u001b[31m"}"#;
    let server = Server::start((0..3).map(|_| response("200 OK", raw)).collect());
    let args = [
        "get-server-console-log",
        "--project-id",
        "proj-1",
        "--server-id",
        "server-1",
        "--output",
        "text",
    ];
    for query in [&[][..], &["--query", "Log"]] {
        let args = [args.as_slice(), query].concat();
        let (status, stdout, stderr) = invoke(&server, &args, true).await;
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, b"boot\nready\t\\x1b[31m");
        assert!(stderr.is_empty());
    }
    let args = [args.as_slice(), &["--query", "length(to_number(Log))"]].concat();
    let (status, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("console log query failed; result withheld"));
}

#[tokio::test]
async fn flavor_price_views_match_go() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = |name: &str| {
        let path = root.join(format!(
            "../greennode-sdk/tests/fixtures/compute/price-{name}.json"
        ));
        response("200 OK", &std::fs::read_to_string(path).unwrap())
    };
    for format in ["json", "table", "text"] {
        let server = Server::start(
            [
                "zones",
                "platforms",
                "families",
                "flavors",
                "disk-groups",
                "disks",
                "images",
                "quote",
            ]
            .into_iter()
            .map(fixture)
            .collect(),
        );
        let mut args = vec![
            "list-flavor-prices",
            "--project-id",
            "proj-1",
            "--read-only",
            "--output",
            format,
            "--zone-id",
            "zone-1",
            "--root-disk-type-id",
            "disk-1",
        ];
        if format == "json" {
            args.extend(["--query", "{Items:Items,Requests:Metadata.Requests}"]);
        }
        let (status, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(status, ExitCode::SUCCESS, "{format}");
        for (suffix, actual) in [("", stdout), (".stderr", stderr)] {
            let golden = root.join(format!(
                "tests/fixtures/compute/list-flavor-prices.{format}{suffix}.golden"
            ));
            assert_eq!(actual, std::fs::read(golden).unwrap(), "{format}{suffix}");
        }
        assert!(server.heads()[7].starts_with(b"POST /portal/v1/price "));
    }
}

#[tokio::test]
async fn quotes_and_server_orders_match_go_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let priced = [
        "--zone-id",
        "zone-1",
        "--flavor-id",
        "flavor-1",
        "--image-id",
        "image-1",
        "--root-disk-size",
        "20",
        "--root-disk-type-id",
        "disk-1",
    ];
    let create = [
        priced.as_slice(),
        &[
            "--name",
            "example",
            "--vpc-id",
            "vpc-1",
            "--subnet-id",
            "subnet-1",
            "--security-group-id",
            "group-1",
            "--ssh-key-id",
            "key-1",
            "--max-price",
            "20",
            "--no-wait",
        ],
    ]
    .concat();
    for (command, flags, routes) in [
        (
            "quote-create-server",
            priced.as_slice(),
            &[("POST /portal/v1/price ", "200 OK", "server-order-quote")][..],
        ),
        (
            "quote-resize-server",
            &["--server-id", "server-1", "--flavor-id", "flavor-2"],
            &[("POST /portal/v1/price ", "200 OK", "server-resize-quote")],
        ),
        (
            "create-server",
            create.as_slice(),
            &[
                (
                    "GET /v2/proj-1/servers?page=1&size=10000 ",
                    "200 OK",
                    "server-order-inventory",
                ),
                ("POST /portal/v1/price ", "200 OK", "server-order-quote"),
                (
                    "POST /v2/proj-1/servers ",
                    "202 Accepted",
                    "server-order-accepted",
                ),
            ],
        ),
        (
            "delete-server",
            &["--server-id", "server-1", "--yes", "--no-wait"],
            &[
                (
                    "GET /v2/proj-1/servers/server-1 ",
                    "200 OK",
                    "server-delete-inventory",
                ),
                (
                    "GET /v2/proj-1/volumes/servers/server-1 ",
                    "200 OK",
                    "server-delete-volumes",
                ),
                ("DELETE /v2/proj-1/servers/server-1 ", "202 Accepted", ""),
            ],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                routes
                    .iter()
                    .map(|(_, status, name)| {
                        let body = if name.is_empty() {
                            String::new()
                        } else {
                            std::fs::read_to_string(root.join(format!(
                                "../greennode-sdk/tests/fixtures/compute/{name}.json"
                            )))
                            .unwrap()
                        };
                        response(status, &body)
                    })
                    .collect(),
            );
            let mut args = vec![command, "--project-id", "proj-1", "--output", format];
            if command.starts_with("quote-") {
                args.push("--read-only");
            }
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&server, &args, false).await;
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            let golden = root.join(format!("tests/fixtures/compute/{command}.{format}.golden"));
            assert_eq!(
                String::from_utf8(stdout).unwrap(),
                std::fs::read_to_string(golden).unwrap(),
                "{command}/{format}"
            );
            let heads = server.heads();
            assert_eq!(heads.len(), routes.len());
            assert!(
                heads
                    .iter()
                    .zip(routes)
                    .all(|(head, (route, _, _))| head.starts_with(route.as_bytes()))
            );
        }
    }
}

#[tokio::test]
async fn delete_server_checks_confirmation_and_read_only_before_network() {
    let server = Server::start(vec![]);
    let args = [
        "delete-server",
        "--project-id",
        "proj-1",
        "--server-id",
        "server-1",
    ];
    for extra in [&[][..], &["--yes", "--read-only"]] {
        let (status, stdout, _) = invoke(&server, &[args.as_slice(), extra].concat(), false).await;
        assert_ne!(status, ExitCode::SUCCESS);
        assert!(stdout.is_empty());
    }
    assert_eq!(server.requests(), 0);
}
