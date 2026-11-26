use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 4","progressStatus":"CREATED"}}"#;
const POOL: &str = r#"{"data":{"uuid":"pool-1","name":"backends","progressStatus":"CREATED","members":[{"address":"192.0.2.10","protocolPort":80,"weight":1}]}}"#;
const MEMBERS: &str = r#"{"data":[{"uuid":"member-1","address":"192.0.2.10","protocolPort":80,"name":"primary","weight":1,"backup":true}]}"#;
const LB: &str = "/v2/proj-1/loadBalancers/lb-1";
const POOL_ROUTE: &str = "/v2/proj-1/loadBalancers/lb-1/pools/pool-1";
const MEMBERS_ROUTE: &str = "/v2/proj-1/loadBalancers/lb-1/pools/pool-1/members";
const REQUIRED: &[&str] = &[
    "--load-balancer-id",
    "lb-1",
    "--pool-id",
    "pool-1",
    "--project-id",
    "proj-1",
    "--port",
    "80",
    "--no-wait",
];

#[tokio::test]
async fn member_writes_match_go_requests_and_output() {
    let original = serde_json::json!({"ipAddress":"192.0.2.10","port":80,"name":"primary","weight":1,"monitorPort":80,"backup":true});
    let new = serde_json::json!({"ipAddress":"192.0.2.11","port":80,"name":"secondary","weight":1,"monitorPort":80,"backup":false});
    let replacement = serde_json::json!({"ipAddress":"192.0.2.10","port":80,"name":"primary","weight":2,"monitorPort":80,"backup":false});
    for (command, flags, expected) in [
        (
            "add-pool-member",
            &["--address", "192.0.2.11", "--name", "secondary"][..],
            vec![original, new],
        ),
        (
            "update-pool-member",
            &["--address", "192.0.2.10", "--backup=false", "--weight", "2"][..],
            vec![replacement],
        ),
        (
            "remove-pool-member",
            &["--address", "192.0.2.10", "--yes"][..],
            vec![],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(vec![
                response("200 OK", PARENT),
                response("200 OK", POOL),
                response("200 OK", MEMBERS),
                response("202 Accepted", ""),
            ]);
            let args = [&[command][..], REQUIRED, flags, &["--output", format]].concat();
            let (exit, stdout, stderr) = invoke(&server, &args, false).await;
            assert_eq!(exit, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(server.requests(), 4);
            for (head, (method, route)) in server.heads().iter().zip([
                ("GET", LB),
                ("GET", POOL_ROUTE),
                ("GET", MEMBERS_ROUTE),
                ("PUT", MEMBERS_ROUTE),
            ]) {
                assert!(head.starts_with(format!("{method} {route} ").as_bytes()));
            }
            let bodies = server.bodies();
            let body: serde_json::Value = serde_json::from_slice(bodies.last().unwrap()).unwrap();
            assert!(
                body == serde_json::json!({"members":expected}),
                "unexpected member replacement body"
            );
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            let golden = root.join(format!(
                "tests/fixtures/loadbalancer/{command}.{format}.golden"
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
        }
    }
}

#[tokio::test]
async fn member_remove_requires_yes_and_read_only_refuses_all_three_writes() {
    let server = Server::start(vec![]);
    let args = [
        &["remove-pool-member"][..],
        REQUIRED,
        &["--address", "192.0.2.10"],
    ]
    .concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("pass --yes"));
    for command in [
        "add-pool-member",
        "update-pool-member",
        "remove-pool-member",
    ] {
        let args = [
            &[command][..],
            REQUIRED,
            &["--address", "192.0.2.10", "--yes", "--read-only"],
        ]
        .concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
    assert_eq!(server.requests(), 0);
}
