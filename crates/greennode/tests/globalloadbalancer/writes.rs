use super::*;
use greennode_sdk::testing::{Product, with_write_loopback};

const ACCEPTED: &str = r#"{"globalLoadBalancer":{"id":"glb-1"}}"#;
const DETAIL: &str =
    r#"{"id":"glb-1","type":"Layer 4","name":"demo","package":"pkg-1","status":"ACTIVE"}"#;
const CREATE: &[&str] = &[
    "create-load-balancer",
    "--cli-input-json",
    r#"{"Name":"old","PackageID":"pkg-1"}"#,
    "--name",
    "demo",
];
const DELETE: &[&str] = &["delete-load-balancer", "--load-balancer-id", "glb-1"];

async fn write(server: &Server, args: &[&str]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_with_options(
        with_write_loopback(Product::GlobalLoadBalancer, options(server, BEARER), server),
        BEARER,
        args,
    )
    .await
}

fn golden(command: &str, format: &str) -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read(root.join(format!(
        "tests/fixtures/globalloadbalancer/{command}.{format}.golden"
    )))
    .unwrap()
}

#[tokio::test]
async fn parent_writes_match_go_success_goldens() {
    for format in ["json", "table", "text"] {
        let create = Server::start(vec![
            response("202 Accepted", ACCEPTED),
            response("200 OK", DETAIL),
        ]);
        let delete = Server::start(vec![
            response("200 OK", DETAIL),
            response("202 Accepted", ""),
            response("404 Not Found", "{}"),
        ]);
        for (args, server, requests) in [(CREATE, &create, 2), (DELETE, &delete, 3)] {
            let args = [args, &["--yes", "--output", format]].concat();
            let (status, stdout, stderr) = write(server, &args).await;
            assert_eq!(status, ExitCode::SUCCESS);
            assert!(stderr.is_empty());
            assert_eq!(stdout, golden(args[0], format));
            assert_eq!(server.requests(), requests);
        }
    }
}

#[tokio::test]
async fn parent_writes_keep_go_partial_output_and_error_identities() {
    let failed = DETAIL.replace("ACTIVE", "ERROR");
    let create = Server::start(vec![
        response("202 Accepted", ACCEPTED),
        response("200 OK", &failed),
    ]);
    let delete = Server::start(vec![
        response("200 OK", DETAIL),
        response("202 Accepted", ""),
        response("403 Forbidden", "{}"),
    ]);
    for (args, server, code) in [
        (CREATE, &create, "WriteFailed"),
        (DELETE, &delete, "NotSettled"),
    ] {
        let args = [args, &["--yes", "--query", "Name"]].concat();
        let (status, stdout, stderr) = write(server, &args).await;
        assert_eq!(status, ExitCode::FAILURE);
        assert_eq!(stdout, golden(args[0], "partial.json"));
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], code);
        assert!(
            envelope["error"]["message"]
                .as_str()
                .unwrap()
                .contains("inspect inventory and billing before another mutation")
        );
        assert!(envelope["error"].get("status").is_none());
        assert!(envelope["error"].get("operation").is_none());
    }
}

#[tokio::test]
async fn parent_writes_enforce_consent_and_read_only_before_requests() {
    let server = Server::start(vec![]);
    for args in [CREATE, DELETE] {
        for (flags, code) in [
            (&[][..], "InvalidUsage"),
            (&["--yes", "--read-only"][..], "ReadOnly"),
        ] {
            let args = [args, flags].concat();
            let (status, stdout, stderr) = write(&server, &args).await;
            assert_eq!(status, ExitCode::from(2));
            assert!(stdout.is_empty());
            let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
            assert_eq!(envelope["error"]["code"], code);
        }
    }
    assert_eq!(server.requests(), 0);
}
