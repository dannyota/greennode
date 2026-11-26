use super::*;

const LB: &str = "/v2/proj-1/loadBalancers/lb-1";
const LB_READY: &str = r#"{"data":{"uuid":"lb-1","progressStatus":"CREATED"}}"#;
const POOL_READY: &str = r#"{"data":{"uuid":"pool-1","progressStatus":"CREATED"}}"#;
const PROJECT: &[&str] = &["--project-id", "proj-1"];
const DELETE_POOL: &[&str] = &[
    "delete-pool",
    "--load-balancer-id",
    "lb-1",
    "--pool-id",
    "pool-1",
];
const DELETE_LISTENER: &[&str] = &[
    "delete-listener",
    "--load-balancer-id",
    "lb-1",
    "--listener-id",
    "listener-1",
];
const DELETE_POLICY: &[&str] = &[
    "delete-policy",
    "--load-balancer-id",
    "lb-1",
    "--listener-id",
    "listener-1",
    "--policy-id",
    "policy-1",
];

#[tokio::test]
async fn child_deletes_require_yes_and_respect_read_only() {
    let server = Server::start(vec![]);
    for command in [DELETE_POOL, DELETE_LISTENER, DELETE_POLICY] {
        let args = [command, PROJECT].concat();
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
async fn child_deletes_match_go_goldens_after_confirming_absence() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (command, path, child) in [
        (DELETE_POOL, "pools/pool-1", POOL_READY),
        (
            DELETE_LISTENER,
            "listeners/listener-1",
            r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED"}}"#,
        ),
        (
            DELETE_POLICY,
            "listeners/listener-1/l7policies/policy-1",
            r#"{"data":{"uuid":"policy-1","progressStatus":"CREATED"}}"#,
        ),
    ] {
        for format in ["json", "table", "text"] {
            let mut plan = Vec::new();
            if command == DELETE_POOL {
                plan.push(("GET", format!("{LB}/listeners"), "200 OK", r#"{"data":[]}"#));
            }
            let route = format!("{LB}/{path}");
            plan.extend([
                ("GET", LB.to_owned(), "200 OK", LB_READY),
                ("GET", route.clone(), "200 OK", child),
                ("DELETE", route.clone(), "202 Accepted", ""),
                ("GET", route, "404 Not Found", "{}"),
                ("GET", LB.to_owned(), "200 OK", LB_READY),
            ]);
            let server = Server::start(
                plan.iter()
                    .map(|(_, _, status, body)| response(status, body))
                    .collect(),
            );
            let args = [command, PROJECT, &["--yes", "--output", format]].concat();
            let (exit, stdout, stderr) = invoke(&server, &args, false).await;
            assert_eq!(exit, ExitCode::SUCCESS, "{}/{format}", command[0]);
            assert!(stderr.is_empty());
            let golden = root.join(format!(
                "tests/fixtures/loadbalancer/{}.{format}.golden",
                command[0]
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{}", command[0]);
            assert_eq!(server.requests(), plan.len());
            for (head, (method, route, _, _)) in server.heads().iter().zip(plan) {
                assert!(head.starts_with(format!("{method} {route} ").as_bytes()));
            }
        }
    }
}

#[tokio::test]
async fn child_pool_delete_refuses_a_listeners_default_pool() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":[{"uuid":"listener_1","defaultPoolId":"pool-1"}]}"#,
    )]);
    let args = [DELETE_POOL, PROJECT, &["--yes"]].concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "ResourceInUse");
    assert_eq!(
        envelope["error"]["message"],
        "loadbalancer: resource in use: loadbalancer.DeletePool: pool pool-1 is used in listener listener_1 as its default pool"
    );
    assert!(envelope["error"].get("status").is_none());
    assert!(envelope["error"].get("operation").is_none());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn child_delete_preserves_partial_when_confirmation_is_denied() {
    let server = Server::start(vec![
        response("200 OK", LB_READY),
        response(
            "200 OK",
            r#"{"data":{"uuid":"listener-1","progressStatus":"CREATED"}}"#,
        ),
        response("202 Accepted", ""),
        response("403 Forbidden", r#"{"message":"lookup denied"}"#),
    ]);
    let args = [DELETE_LISTENER, PROJECT, &["--yes"]].concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(1));
    assert_eq!(
        stdout,
        include_bytes!("../fixtures/loadbalancer/delete-listener.partial.json.golden")
    );
    assert_eq!(
        stderr,
        include_bytes!("../fixtures/loadbalancer/delete-listener.partial.stderr.golden")
    );
    assert_eq!(server.requests(), 4);
}
