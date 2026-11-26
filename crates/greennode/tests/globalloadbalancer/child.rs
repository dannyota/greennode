use super::*;
use greennode_sdk::testing::{Product, with_product_loopback, with_write_loopback};
use serde_json::{Value, json};

const FIXTURE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/globalloadbalancer/child_writes.json");
const CREATE_POOL: &[&str] = &[
    "create-pool",
    "--load-balancer-id",
    "glb-1",
    "--name",
    "demo-pool",
    "--member-name",
    "backend",
    "--backend-server-id",
    "server-1",
    "--backend-vpc-id",
    "vpc-1",
    "--backend-subnet-id",
    "sub-1",
    "--backend-port",
    "8080",
    "--project-id",
    "project-1",
];
const CREATE_LISTENER: &[&str] = &[
    "create-listener",
    "--load-balancer-id",
    "glb-1",
    "--name",
    "demo-listener",
    "--pool-id",
    "pool-1",
    "--port",
    "443",
    "--allowed-cidrs",
    "192.0.2.1/32",
    "--allowed-cidrs",
    "198.51.100.2/32",
    "--cli-input-json",
    r#"{"BackendProjectID":"old-project","AllowedCIDRs":["0.0.0.0/0"]}"#,
    "--project-id",
    "project-1",
    "--backend-server-id",
    "server-1",
];
const DELETE_POOL: &[&str] = &[
    "delete-pool",
    "--load-balancer-id",
    "glb-1",
    "--pool-id",
    "pool-1",
];
const DELETE_LISTENER: &[&str] = &[
    "delete-listener",
    "--load-balancer-id",
    "glb-1",
    "--listener-id",
    "listener-1",
];

async fn invoke_child(
    glb: &Server,
    regional: &Server,
    args: &[&str],
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let options = with_write_loopback(Product::GlobalLoadBalancer, options(glb, BEARER), glb);
    let options = with_product_loopback(Product::Vserver, options, regional);
    invoke_with_options(options, BEARER, &[args, &["--region", "han-1"]].concat()).await
}

fn golden(command: &str, format: &str) -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read(root.join(format!(
        "tests/fixtures/globalloadbalancer/{command}.{format}.golden"
    )))
    .unwrap()
}

fn servers(command: &str, failed: bool) -> (Server, Server, usize, usize) {
    let f: Value = serde_json::from_str(FIXTURE).unwrap();
    let ok = |key: &str| response("200 OK", &f[key].to_string());
    let list = |key: &str| response("200 OK", &json!([f[key]]).to_string());
    let empty = || response("200 OK", "[]");
    let accept = |key: &str| response("202 Accepted", &f[key].to_string());
    let accepted = || response("202 Accepted", "");
    let not_found = || response("404 Not Found", "{}");
    let chain = || [list("pool"), ok("pool"), list("group"), ok("group")];
    let mut glb = vec![ok("parent"), ok("regions"), ok("packages")];
    let mut regional = Vec::new();
    match command {
        "create-pool" => {
            glb.extend([empty(), empty(), accept("pool_accept")]);
            if failed {
                glb.push(not_found());
            } else {
                glb.push(ok("parent"));
                glb.extend(chain());
                glb.push(empty());
            }
            regional.extend([ok("server"), ok("vpc"), ok("subnet")]);
        }
        "create-listener" => {
            glb.push(empty());
            glb.extend(chain());
            glb.push(accept("listener_accept"));
            if failed {
                glb.push(not_found());
            } else {
                glb.extend([ok("listener"), list("listener")]);
                glb.extend(chain());
            }
            regional.extend([ok("server"), ok("vpc"), ok("subnet")]);
        }
        "delete-pool" => {
            glb.extend([
                list("pool"),
                ok("pool"),
                empty(),
                list("group"),
                ok("group"),
                accepted(),
                empty(),
            ]);
            if failed {
                glb.push(response("403 Forbidden", "{}"));
            } else {
                glb.extend([not_found(), ok("parent"), not_found()]);
            }
        }
        "delete-listener" => {
            glb.extend([ok("listener"), list("listener")]);
            glb.extend(chain());
            glb.extend([accepted(), empty()]);
            if failed {
                glb.push(response("403 Forbidden", "{}"));
            } else {
                glb.extend([
                    not_found(),
                    ok("parent"),
                    list("pool"),
                    ok("pool"),
                    list("group"),
                ]);
            }
        }
        _ => panic!("unknown command"),
    }
    let counts = (glb.len(), regional.len());
    (
        Server::start(glb),
        Server::start(regional),
        counts.0,
        counts.1,
    )
}

#[tokio::test]
async fn all_child_commands_match_go_success_and_partial_goldens() {
    for args in [CREATE_POOL, CREATE_LISTENER, DELETE_POOL, DELETE_LISTENER] {
        let command = args[0];
        for format in ["json", "table", "text", "partial.json"] {
            let failed = format == "partial.json";
            let args = if failed {
                [args, &["--yes", "--query", "ID"]].concat()
            } else {
                [args, &["--yes", "--output", format]].concat()
            };
            let (glb, regional, glb_count, regional_count) = servers(command, failed);
            let (status, stdout, stderr) = invoke_child(&glb, &regional, &args).await;
            if failed {
                assert_eq!(status, ExitCode::FAILURE);
                let error: Value = serde_json::from_slice(&stderr).unwrap();
                assert_eq!(error["error"]["code"], "NotSettled");
                assert!(error["error"].get("operation").is_none());
                assert!(error["error"].get("status").is_none());
            } else {
                assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
                assert!(stderr.is_empty());
            }
            assert_eq!(stdout, golden(command, format));
            assert_eq!(glb.requests(), glb_count);
            assert_eq!(regional.requests(), regional_count);
            assert!(glb.heads().iter().all(|head| {
                head.windows(b"/glb-controller/v1/".len())
                    .any(|window| window == b"/glb-controller/v1/")
            }));
            assert!(
                regional
                    .heads()
                    .iter()
                    .all(|head| head.starts_with(b"GET /v2/project-1/"))
            );
        }
    }
}

#[tokio::test]
async fn child_commands_require_consent_and_latch_read_only_before_io() {
    let glb = Server::start(vec![]);
    let regional = Server::start(vec![]);
    for args in [CREATE_POOL, CREATE_LISTENER, DELETE_POOL, DELETE_LISTENER] {
        for (suffix, code) in [
            (&[][..], "InvalidUsage"),
            (&["--read-only", "--yes"][..], "ReadOnly"),
        ] {
            let args = [args, suffix].concat();
            let (status, stdout, stderr) = invoke_child(&glb, &regional, &args).await;
            assert_eq!(status, ExitCode::from(2));
            assert!(stdout.is_empty());
            let error: Value = serde_json::from_slice(&stderr).unwrap();
            assert_eq!(error["error"]["code"], code);
        }
    }
    assert_eq!(glb.requests(), 0);
    assert_eq!(regional.requests(), 0);
}
