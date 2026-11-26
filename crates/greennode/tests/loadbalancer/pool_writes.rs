use super::*;

const LB: &str = "/v2/proj-1/loadBalancers/lb-1";
const POOL: &str = "/v2/proj-1/loadBalancers/lb-1/pools/pool-1";
const LB_READY: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;
const POOL_READY: &str = r#"{"data":{"uuid":"pool-1","name":"web","protocol":"HTTP","loadBalanceMethod":"ROUND_ROBIN","progressStatus":"CREATED"}}"#;
const MONITOR: &str = r#"{"data":{"healthCheckProtocol":"HTTP","healthyThreshold":4,"unhealthyThreshold":5,"interval":30,"timeout":5,"domainName":"example.test"}}"#;
const CREATE_ARGS: &[&str] = &[
    "create-pool",
    "--load-balancer-id",
    "lb-1",
    "--name",
    "web",
    "--protocol",
    "HTTP",
    "--health-check-protocol",
    "HTTP",
    "--no-wait",
    "--project-id",
    "proj-1",
];
const UPDATE_ARGS: &[&str] = &[
    "update-pool",
    "--load-balancer-id",
    "lb-1",
    "--pool-id",
    "pool-1",
    "--algorithm",
    "LEAST_CONNECTIONS",
    "--stickiness=false",
    "--no-wait",
    "--project-id",
    "proj-1",
];

#[tokio::test]
async fn pool_create_sends_http_defaults_and_returns_go_output() {
    for format in ["json", "table", "text"] {
        let server = Server::start(vec![
            response("200 OK", LB_READY),
            response("202 Accepted", r#"{"uuid":"pool-1"}"#),
        ]);
        let args = [CREATE_ARGS, &["--output", format]].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::SUCCESS, "{format}");
        assert!(stderr.is_empty());
        let expected = serde_json::json!({"poolName":"web","poolProtocol":"HTTP","algorithm":"ROUND_ROBIN","stickiness":false,"tlsEncryption":false,"healthMonitor":{"healthCheckProtocol":"HTTP","healthyThreshold":3,"unhealthyThreshold":3,"interval":30,"timeout":5,"healthCheckPath":"/","healthCheckMethod":"GET","httpVersion":"1.1","successCode":"200"}});
        assert_requests(
            &server,
            &[("GET", LB), ("POST", &format!("{LB}/pools"))],
            expected,
        );
        assert_golden("create-pool", format, &stdout);
    }
}

#[tokio::test]
async fn pool_update_merges_current_monitor_and_preserves_explicit_false() {
    for format in ["json", "table", "text"] {
        let server = Server::start(vec![
            response("200 OK", LB_READY),
            response("200 OK", POOL_READY),
            response("200 OK", MONITOR),
            response("202 Accepted", ""),
        ]);
        let args = [UPDATE_ARGS, &["--output", format]].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::SUCCESS, "{format}");
        assert!(stderr.is_empty());
        let expected = serde_json::json!({"poolName":"","poolProtocol":"","algorithm":"LEAST_CONNECTIONS","stickiness":false,"tlsEncryption":false,"healthMonitor":{"healthyThreshold":4,"unhealthyThreshold":5,"interval":30,"timeout":5,"healthCheckPath":"/","healthCheckMethod":"GET","httpVersion":"1.1","domainName":"example.test","successCode":"200"}});
        assert_requests(
            &server,
            &[
                ("GET", LB),
                ("GET", POOL),
                ("GET", &format!("{POOL}/healthMonitor")),
                ("PUT", POOL),
            ],
            expected,
        );
        assert_golden("update-pool", format, &stdout);
    }
}

#[tokio::test]
async fn pool_writes_respect_read_only_and_validate_inputs_before_reads() {
    let server = Server::start(vec![]);
    for command in [CREATE_ARGS, UPDATE_ARGS] {
        let args = [command, &["--read-only"]].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
    let args = [
        "update-pool",
        "--load-balancer-id",
        "lb-1",
        "--pool-id",
        "pool-1",
        "--project-id",
        "proj-1",
    ];
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("requires at least one field"));
    assert_eq!(server.requests(), 0);
}

fn assert_requests(server: &Server, plan: &[(&str, &str)], expected: serde_json::Value) {
    assert_eq!(server.requests(), plan.len());
    for (head, (method, route)) in server.heads().iter().zip(plan) {
        assert!(head.starts_with(format!("{method} {route} ").as_bytes()));
    }
    let bodies = server.bodies();
    let actual: serde_json::Value = serde_json::from_slice(bodies.last().unwrap()).unwrap();
    assert!(actual == expected, "unexpected pool request body");
}

fn assert_golden(command: &str, format: &str, stdout: &[u8]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join(format!(
        "tests/fixtures/loadbalancer/{command}.{format}.golden"
    ));
    assert_eq!(stdout, std::fs::read(path).unwrap(), "{command}/{format}");
}
