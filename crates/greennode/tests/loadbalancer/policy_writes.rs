use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;
const CHILD: &str = r#"{"data":{"uuid":"policy-1","name":"routing","action":"REDIRECT_TO_POOL","redirectPoolId":"pool-1","progressStatus":"CREATED","l7Rules":[{"uuid":"rule-1","compareType":"STARTS_WITH","ruleValue":"/api","ruleType":"PATH"}]}}"#;
const LB: &str = "/v2/proj-1/loadBalancers/lb-1";
const COLLECTION: &str = "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies";
const DETAIL: &str = "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1/l7policies/policy-1";
const CREATE: &[&str] = &[
    "create-policy",
    "--load-balancer-id",
    "lb-1",
    "--listener-id",
    "listener-1",
    "--project-id",
    "proj-1",
    "--no-wait",
];
const UPDATE: &[&str] = &[
    "update-policy",
    "--load-balancer-id",
    "lb-1",
    "--listener-id",
    "listener-1",
    "--policy-id",
    "policy-1",
    "--project-id",
    "proj-1",
    "--no-wait",
];

#[tokio::test]
async fn policy_create_and_update_use_the_merged_action_and_go_projection() {
    for format in ["json", "table", "text"] {
        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("202 Accepted", r#"{"uuid":"policy-1"}"#),
        ]);
        let args = [
            CREATE,
            &[
                "--name",
                "routing",
                "--action",
                "REDIRECT_TO_POOL",
                "--redirect-pool-id",
                "pool-1",
                "--cli-input-json",
                r#"{"Rules":[{"Type":"PATH","CompareType":"STARTS_WITH","Value":"/api"}]}"#,
                "--output",
                format,
            ],
        ]
        .concat();
        let expected = serde_json::json!({"name":"routing","action":"REDIRECT_TO_POOL","redirectPoolId":"pool-1","rules":[{"ruleType":"PATH","compareType":"STARTS_WITH","ruleValue":"/api"}]});
        assert_write(
            &server,
            &args,
            format,
            &[("GET", LB), ("POST", COLLECTION)],
            expected,
        )
        .await;

        let server = Server::start(vec![
            response("200 OK", PARENT),
            response("200 OK", CHILD),
            response("202 Accepted", ""),
        ]);
        let args = [
            UPDATE,
            &[
                "--action",
                "REDIRECT_TO_URL",
                "--redirect-url",
                "https://example.test/api",
                "--redirect-http-code",
                "302",
                "--keep-query-string=false",
                "--output",
                format,
            ],
        ]
        .concat();
        let expected = serde_json::json!({"action":"REDIRECT_TO_URL","redirectUrl":"https://example.test/api","redirectHttpCode":302,"keepQueryString":false,"rules":[{"ruleType":"PATH","compareType":"STARTS_WITH","ruleValue":"/api"}]});
        assert_write(
            &server,
            &args,
            format,
            &[("GET", LB), ("GET", DETAIL), ("PUT", DETAIL)],
            expected,
        )
        .await;
    }
}

#[tokio::test]
async fn policy_write_guards_reject_empty_rules_and_read_only() {
    let server = Server::start(vec![]);
    for (base, options, expected) in [
        (
            CREATE,
            &[
                "--name",
                "routing",
                "--action",
                "REDIRECT_TO_POOL",
                "--redirect-pool-id",
                "pool-1",
            ][..],
            "requires at least one rule",
        ),
        (
            UPDATE,
            &["--cli-input-json", r#"{"Rules":[]}"#][..],
            "requires at least one rule",
        ),
        (
            UPDATE,
            &["--keep-query-string", "--read-only"][..],
            "ReadOnly",
        ),
    ] {
        let args = [base, options].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&stderr).contains(expected),
            "missing {expected}"
        );
    }
    assert_eq!(server.requests(), 0);
}

async fn assert_write(
    server: &Server,
    args: &[&str],
    format: &str,
    plan: &[(&str, &str)],
    expected: serde_json::Value,
) {
    let (exit, stdout, stderr) = invoke(server, args, false).await;
    assert_eq!(exit, ExitCode::SUCCESS, "{}/{format}", args[0]);
    assert!(stderr.is_empty());
    assert_eq!(server.requests(), plan.len());
    for (head, (method, path)) in server.heads().iter().zip(plan) {
        assert!(head.starts_with(format!("{method} {path} ").as_bytes()));
    }
    let bodies = server.bodies();
    let actual: serde_json::Value = serde_json::from_slice(bodies.last().unwrap()).unwrap();
    assert!(actual == expected, "unexpected policy request body");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let golden = root.join(format!(
        "tests/fixtures/loadbalancer/{}.{format}.golden",
        args[0]
    ));
    assert_eq!(
        stdout,
        std::fs::read(golden).unwrap(),
        "{}/{format}",
        args[0]
    );
}

#[tokio::test]
async fn policy_json_refuses_unknown_rule_fields_before_any_request() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"policy-1"}"#),
    ]);
    let args = [CREATE, &["--name", "routing", "--action", "REDIRECT_TO_POOL", "--redirect-pool-id", "pool-1", "--cli-input-json", r#"{"Rules":[{"Type":"PATH","CompareType":"STARTS_WITH","Value":"/api","Unexpected":"two"}]}"#]].concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("--cli-input-json"));
    assert_eq!(server.requests(), 0);
}
