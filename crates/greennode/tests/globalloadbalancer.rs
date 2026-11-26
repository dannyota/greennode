use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "synthetic-glb-cli-bearer";

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

async fn invoke(server: &Server, bearer: &str, args: &[&str]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let status = run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "globalloadbalancer"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_billing_loopback(
            ConfigOptions::default()
                .with_static_token(Secret::new(bearer))
                .with_endpoint_overrides(EndpointOverrides {
                    global_load_balancer: Some(format!("{}/glb-controller/", server.origin)),
                    ..Default::default()
                }),
            server,
        ),
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    })
    .await;
    assert!(!String::from_utf8_lossy(&stdout).contains(bearer));
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    (status, stdout, stderr)
}

#[tokio::test]
async fn reads_match_all_go_goldens_using_the_global_endpoint() {
    let parent = ["--load-balancer-id", "glb-1"];
    let pool = ["--load-balancer-id", "glb-1", "--pool-id", "pool-1"];
    for (command, path, flags) in [
        ("list-packages", "packages", &[][..]),
        ("list-regions", "regions", &[]),
        (
            "list-load-balancers",
            "global-load-balancers?limit=10&name=app+%2B+blue&offset=2",
            &[
                "--cli-input-json",
                r#"{"Limit":10,"Name":"app + blue","Offset":1}"#,
                "--offset",
                "2",
            ],
        ),
        ("get-load-balancer", "global-load-balancers/glb-1", &parent),
        (
            "list-pools",
            "global-load-balancers/glb-1/global-pools",
            &parent,
        ),
        (
            "list-listeners",
            "global-load-balancers/glb-1/global-listeners",
            &parent,
        ),
        (
            "get-listener",
            "global-load-balancers/glb-1/global-listeners/listener-1",
            &["--load-balancer-id", "glb-1", "--listener-id", "listener-1"],
        ),
        (
            "list-pool-members",
            "global-load-balancers/glb-1/global-pools/pool-1/pool-members",
            &pool,
        ),
        (
            "get-pool-member",
            "global-load-balancers/glb-1/global-pools/pool-1/pool-members/member-1",
            &[
                "--load-balancer-id",
                "glb-1",
                "--pool-id",
                "pool-1",
                "--pool-member-id",
                "member-1",
            ],
        ),
        (
            "list-usage-histories",
            "global-load-balancers/glb-1/usage-histories?from=toy+%2B+start&to=toy-end&type=traffic",
            &[
                "--load-balancer-id",
                "glb-1",
                "--from",
                "toy + start",
                "--to",
                "toy-end",
                "--type",
                "traffic",
            ],
        ),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = root.join(format!(
            "../greennode-sdk/tests/fixtures/globalloadbalancer/{}.json",
            command.replace('-', "_")
        ));
        let body = std::fs::read_to_string(fixture).unwrap();
        let server = Server::start((0..3).map(|_| response("200 OK", &body)).collect());
        for format in ["json", "table", "text"] {
            let mut args = vec![
                command,
                "--read-only",
                "--project-id",
                "unused-project",
                "--output",
                format,
            ];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&server, BEARER, &args).await;
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            let golden = root.join(format!(
                "tests/fixtures/globalloadbalancer/{command}.{format}.golden"
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
        }
        assert_eq!(server.requests(), 3);
        assert!(server.heads().iter().all(|head| {
            head.starts_with(format!("GET /glb-controller/v1/{path} ").as_bytes())
                && !head
                    .windows(b"unused-project".len())
                    .any(|part| part == b"unused-project")
        }));
    }
}

#[tokio::test]
async fn required_ids_stop_before_a_request() {
    let server = Server::start(vec![]);
    for (args, field) in [
        (&["get-load-balancer"][..], "load-balancer-id"),
        (
            &["get-listener", "--load-balancer-id", "glb-1"][..],
            "listener-id",
        ),
        (
            &[
                "get-pool-member",
                "--load-balancer-id",
                "glb-1",
                "--pool-id",
                "pool-1",
            ][..],
            "pool-member-id",
        ),
    ] {
        let (status, stdout, stderr) = invoke(&server, BEARER, args).await;
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "InvalidUsage");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .unwrap()
                .contains(field)
        );
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn reflected_strings_and_map_keys_are_removed_before_rendering() {
    let escaped = BEARER.replace('-', "\\u002d");
    let body = format!(
        r#"[{{"name":"prefix-{escaped}","detail":{{"{escaped}":"drop","nested":[{{"keep":"{escaped}","apiKey":"another-toy-secret","label":"public"}}]}}}},{{"detail":[{{"apiKey":"another-toy-secret","number":9007199254740993}}]}}]"#
    );
    let server = Server::start((0..4).map(|_| response("200 OK", &body)).collect());
    for format in ["json", "table", "text"] {
        let (status, stdout, stderr) =
            invoke(&server, BEARER, &["list-packages", "--output", format]).await;
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("redacted"));
        assert!(stdout.contains("public"));
        assert!(!stdout.contains("another-toy-secret"));
        assert!(!stdout.contains("drop"));
        if format == "json" {
            assert!(stdout.contains("\"number\": 9007199254740992"));
        }
    }
    let (status, stdout, _) = invoke(
        &server,
        BEARER,
        &["list-packages", "--query", "keys(Items[0].Detail)"],
    )
    .await;
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stdout).unwrap(),
        serde_json::json!(["nested"])
    );
}
