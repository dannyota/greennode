use super::*;

const PARENT: &str = r#"{"data":{"uuid":"lb-1","type":"Layer 7","progressStatus":"CREATED"}}"#;
const LISTENER: &str = r#"{"data":{"uuid":"listener-1","name":"web","protocol":"HTTPS","protocolPort":443,"defaultPoolId":"pool-1","timeoutClient":51,"timeoutMember":52,"timeoutConnection":6,"allowedCidrs":"10.0.0.0/24","certificateAuthorities":["cert-1"],"defaultCertificateAuthority":"cert-1","clientCertificateAuthentication":"client-1","progressStatus":"CREATED","insertHeaders":[{"headerName":"X-To","headerValue":"web"}]}}"#;
const ROOT: &str = "/v2/proj-1/loadBalancers/lb-1";
const COLLECTION: &str = "/v2/proj-1/loadBalancers/lb-1/listeners";
const DETAIL: &str = "/v2/proj-1/loadBalancers/lb-1/listeners/listener-1";
const CREATE: &[&str] = &[
    "create-listener",
    "--load-balancer-id",
    "lb-1",
    "--name",
    "web",
    "--protocol",
    "HTTPS",
    "--port",
    "443",
    "--default-certificate-id",
    "cert-1",
    "--project-id",
    "proj-1",
    "--no-wait",
];
const UPDATE: &[&str] = &[
    "update-listener",
    "--load-balancer-id",
    "lb-1",
    "--listener-id",
    "listener-1",
    "--project-id",
    "proj-1",
    "--no-wait",
];

#[tokio::test]
async fn listener_writes_match_go_bodies_and_goldens() {
    for format in ["json", "table", "text"] {
        let create = Server::start(vec![
            response("200 OK", PARENT),
            response("202 Accepted", r#"{"uuid":"listener-1"}"#),
        ]);
        let args = [CREATE, &[
            "--allowed-cidrs", "10.0.0.0/24, 100.64.0.0/10", "--cli-input-json",
            r#"{"AllowedCIDRs":["0.0.0.0/0"],"CertificateIDs":["cert-1"],"InsertHeaders":[{"HeaderName":"X-To","HeaderValue":"web"}]}"#,
            "--output", format,
        ]].concat();
        let expected = serde_json::json!({"listenerName":"web","listenerProtocol":"HTTPS","listenerProtocolPort":443,"allowedCidrs":"10.0.0.0/24,100.64.0.0/10","timeoutClient":50,"timeoutMember":50,"timeoutConnection":5,"defaultCertificateAuthority":"cert-1","certificateAuthorities":["cert-1"],"insertHeaders":[{"headerName":"X-To","headerValue":"web"}]});
        assert_write(
            &create,
            &args,
            format,
            &[("GET", ROOT), ("POST", COLLECTION)],
            expected,
        )
        .await;

        let update = Server::start(vec![
            response("200 OK", PARENT),
            response("200 OK", LISTENER),
            response("200 OK", ""),
        ]);
        let args = [
            UPDATE,
            &[
                "--timeout-client",
                "0",
                "--client-certificate-id",
                "",
                "--output",
                format,
            ],
        ]
        .concat();
        let expected = serde_json::json!({"allowedCidrs":"10.0.0.0/24","timeoutClient":0,"timeoutMember":52,"timeoutConnection":6,"defaultPoolId":"pool-1","defaultCertificateAuthority":"cert-1","certificateAuthorities":["cert-1"],"insertHeaders":[{"headerName":"X-To","headerValue":"web"}]});
        assert_write(
            &update,
            &args,
            format,
            &[("GET", ROOT), ("GET", DETAIL), ("PUT", DETAIL)],
            expected,
        )
        .await;
    }
}

#[tokio::test]
async fn listener_cidr_and_read_only_guards_run_before_requests() {
    let server = Server::start(vec![]);
    for (base, options, expected) in [
        (CREATE, &[][..], "--allowed-cidrs is required"),
        (
            CREATE,
            &["--allowed-cidrs", "100.64.0.0/9"][..],
            "pass --yes",
        ),
        (
            UPDATE,
            &["--cli-input-json", r#"{"AllowedCIDRs":["0.0.0.0/0"]}"#][..],
            "pass --yes",
        ),
        (
            UPDATE,
            &["--allowed-cidrs", ""][..],
            "requires at least one AllowedCIDRs entry",
        ),
        (
            CREATE,
            &["--allowed-cidrs", "0.0.0.0/0", "--yes", "--read-only"][..],
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
    assert!(actual == expected, "unexpected listener request body");
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
async fn listener_public_cidr_accepts_explicit_yes() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"listener-1"}"#),
    ]);
    let args = [CREATE, &["--allowed-cidrs", "203.0.113.0/28", "--yes"]].concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(value["Listener"]["AllowedCIDRs"], "203.0.113.0/28");
    assert_eq!(server.requests(), 2);
}

#[tokio::test]
async fn listener_json_refuses_unknown_insert_header_fields_before_any_request() {
    let server = Server::start(vec![
        response("200 OK", PARENT),
        response("202 Accepted", r#"{"uuid":"listener-1"}"#),
    ]);
    let args = [
        CREATE,
        &[
            "--allowed-cidrs",
            "10.0.0.0/24",
            "--cli-input-json",
            r#"{"InsertHeaders":[{"HeaderName":"X-To","HeaderValue":"one","Unexpected":"two"}]}"#,
        ],
    ]
    .concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("--cli-input-json"));
    assert_eq!(server.requests(), 0);
}
