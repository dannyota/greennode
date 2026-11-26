#[cfg(unix)]
#[path = "compute/ssh_keys.rs"]
mod ssh_keys;

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
    invoke_with_options(
        server,
        args,
        terminal,
        ConfigOptions::default().with_static_token(Secret::new(BEARER)),
    )
    .await
}

async fn invoke_with_options(
    server: &Server,
    args: &[&str],
    terminal: bool,
    options: ConfigOptions,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
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
            options.with_endpoint_overrides(EndpointOverrides {
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
async fn create_server_rejects_empty_user_data_file_before_network() {
    let path = std::env::temp_dir().join(format!(
        "greennode-compute-empty-user-data-{}",
        std::process::id()
    ));
    std::fs::write(&path, "").unwrap();
    let server = Server::start(vec![]);
    let args = [
        "create-server",
        "--project-id",
        "proj-1",
        "--name",
        "example",
        "--zone-id",
        "zone-1",
        "--flavor-id",
        "flavor-1",
        "--image-id",
        "image-1",
        "--vpc-id",
        "vpc-1",
        "--subnet-id",
        "subnet-1",
        "--security-group-id",
        "group-1",
        "--ssh-key-id",
        "key-1",
        "--root-disk-size",
        "20",
        "--root-disk-type-id",
        "disk-1",
        "--max-price",
        "20",
        "--user-data-file",
        path.to_str().unwrap(),
    ];
    let (status, stdout, _) = invoke(&server, &args, false).await;
    std::fs::remove_file(path).unwrap();
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(server.requests(), 0);
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

#[tokio::test]
async fn server_writes_match_go_goldens() {
    type Route<'a> = (&'a str, &'a str, &'a str);
    const ACTIVE: Route<'_> = (
        "GET /v2/proj-1/servers/server-1 ",
        "200 OK",
        "server-action-active",
    );
    const STOPPED: Route<'_> = (
        "GET /v2/proj-1/servers/server-1 ",
        "200 OK",
        "server-action-stopped",
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (command, flags, routes) in [
        (
            "start-server",
            &["--no-wait"][..],
            &[
                STOPPED,
                ("PUT /v2/proj-1/servers/server-1/start ", "202 Accepted", ""),
            ][..],
        ),
        (
            "stop-server",
            &["--no-wait", "--yes"],
            &[
                ACTIVE,
                ("PUT /v2/proj-1/servers/server-1/stop ", "202 Accepted", ""),
            ],
        ),
        (
            "reboot-server",
            &["--no-wait", "--yes"],
            &[
                ACTIVE,
                (
                    "PUT /v2/proj-1/servers/server-1/reboot ",
                    "202 Accepted",
                    "",
                ),
            ],
        ),
        (
            "rename-server",
            &["--name", "renamed"],
            &[(
                "PUT /v2/proj-1/servers/server-1/rename ",
                "200 OK",
                "server-action-renamed",
            )],
        ),
        (
            "resize-server",
            &["--flavor-id", "flavor-2", "--no-wait", "--yes"],
            &[
                ACTIVE,
                ("POST /portal/v1/price ", "200 OK", "server-resize-quote"),
                (
                    "PUT /v2/proj-1/servers/server-1/resize ",
                    "202 Accepted",
                    "",
                ),
            ],
        ),
        (
            "create-server-group",
            &[
                "--name",
                "example",
                "--policy-id",
                "policy-1",
                "--description",
                "toy",
            ],
            &[(
                "POST /v2/proj-1/serverGroups ",
                "201 Created",
                "group-result",
            )],
        ),
        (
            "update-server-group",
            &["--server-group-id", "group-1", "--description", "toy"],
            &[
                (
                    "GET /v2/proj-1/serverGroups/group-1 ",
                    "200 OK",
                    "group-current",
                ),
                ("PUT /v2/proj-1/serverGroups/group-1 ", "200 OK", ""),
                (
                    "GET /v2/proj-1/serverGroups/group-1 ",
                    "200 OK",
                    "group-result",
                ),
            ],
        ),
        (
            "delete-server-group",
            &["--server-group-id", "group-1", "--yes"],
            &[
                (
                    "GET /v2/proj-1/serverGroups?limit=10000&name=&offset=0 ",
                    "200 OK",
                    "group-empty-inventory",
                ),
                (
                    "DELETE /v2/proj-1/serverGroups/group-1 ",
                    "204 No Content",
                    "",
                ),
            ],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                routes
                    .iter()
                    .map(|(_, status, body)| {
                        let body = if body.is_empty() {
                            String::new()
                        } else {
                            std::fs::read_to_string(root.join(format!(
                                "../greennode-sdk/tests/fixtures/compute/{body}.json"
                            )))
                            .unwrap()
                        };
                        response(status, &body)
                    })
                    .collect(),
            );
            let mut args = vec![command, "--project-id", "proj-1", "--output", format];
            if !command.ends_with("-group") {
                args.extend(["--server-id", "server-1"]);
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
async fn disruptive_server_actions_require_confirmation_and_refuse_read_only() {
    let server = Server::start(vec![]);
    for (command, flags) in [
        ("stop-server", &[][..]),
        ("reboot-server", &[]),
        ("resize-server", &["--flavor-id", "flavor-2"]),
    ] {
        for guard in [&[][..], &["--yes", "--read-only"]] {
            let args = [
                &[command, "--project-id", "proj-1", "--server-id", "server-1"],
                flags,
                guard,
            ]
            .concat();
            let (status, stdout, _) = invoke(&server, &args, false).await;
            assert_eq!(status, ExitCode::from(2));
            assert!(stdout.is_empty());
        }
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn start_server_oversized_not_found_returns_exit_four_without_partial() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"uuid":"server-1","status":"STOPPED"}}"#,
        ),
        response("404 Not Found", &"x".repeat((8 << 20) + 1)),
    ]);
    let (exit, stdout, stderr) = invoke(
        &server,
        &[
            "start-server",
            "--project-id",
            "proj-1",
            "--server-id",
            "server-1",
            "--no-wait",
        ],
        false,
    )
    .await;
    assert_eq!(exit, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
}

#[tokio::test]
async fn group_delete_in_use_404_keeps_provider_status_exit() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":1,"totalItem":0}"#,
        ),
        response("404 Not Found", r#"{"message":"server group is in use"}"#),
    ]);
    let (exit, stdout, stderr) = invoke(
        &server,
        &[
            "delete-server-group",
            "--project-id",
            "proj-1",
            "--server-group-id",
            "group-1",
            "--yes",
        ],
        false,
    )
    .await;
    assert_eq!(exit, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "ServerGroupInUse");
}

#[tokio::test]
async fn lifecycle_preflight_refusals_use_service_identities_without_http_metadata() {
    for (command, flags, body, code) in [
        (
            "start-server",
            &["--server-id", "server-1"][..],
            r#"{"data":{"uuid":"server-1","status":"CREATING"}}"#,
            "UnexpectedStatus",
        ),
        (
            "delete-server-group",
            &["--server-group-id", "group-1", "--yes"],
            r#"{"listData":[{"uuid":"group-1","servers":[{"uuid":"server-1"}]}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#,
            "ServerGroupInUse",
        ),
    ] {
        let server = Server::start(vec![response("200 OK", body)]);
        let args = [&[command, "--project-id", "proj-1"][..], flags].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(1));
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        let error = envelope["error"].as_object().unwrap();
        assert_eq!(error["code"], code);
        assert_eq!(error.len(), 2);
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
        assert_eq!(server.requests(), 1);
    }
}
