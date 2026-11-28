use super::*;

const GROUP: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-floating-group.json");
const SAFE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-floating-rules.json");
const QUOTE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-order-quote.json");
const INVENTORY: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-order-inventory.json");
const ACK: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-order-accepted.json");
const ACTIVE: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/server-floating-active.json");

const CREATE: &[&str] = &[
    "create-server",
    "--project-id",
    "proj-1",
    "--name",
    "web-1",
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
    "sg-1",
    "--ssh-key-id",
    "key-1",
    "--root-disk-size",
    "20",
    "--root-disk-type-id",
    "disk-1",
    "--max-price",
    "20",
    "--attach-floating-ip",
];

fn floating_server() -> Server {
    Server::start(vec![
        response("200 OK", INVENTORY),
        response("200 OK", GROUP),
        response("200 OK", SAFE),
        response("200 OK", QUOTE),
        response("200 OK", GROUP),
        response("200 OK", SAFE),
        response("202 Accepted", ACK),
        response("200 OK", ACTIVE),
    ])
}

#[tokio::test]
async fn floating_create_matches_the_go_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for format in ["json", "table", "text"] {
        let server = floating_server();
        let args = [CREATE, &["--output", format]].concat();
        let (exit, stdout, stderr) = invoke(&server, &args, false).await;
        assert_eq!(exit, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let golden = root.join(format!(
            "tests/fixtures/compute/create-server-floating-ip.{format}.golden"
        ));
        assert_eq!(stdout, std::fs::read(golden).unwrap(), "{format}");
        assert_floating_routes(&server);
    }
    let server = floating_server();
    let args = [CREATE, &["--output", "text", "--query", "Server.PublicIP"]].concat();
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert_eq!(
        stdout,
        include_bytes!(
            "../fixtures/render/golden/compute-create-server-floating-ip-query.text.golden"
        )
    );
    assert_floating_routes(&server);
}

fn assert_floating_routes(server: &Server) {
    let routes = [
        "GET /v2/proj-1/servers?page=1&size=10000 ",
        "GET /v2/proj-1/secgroups/sg-1 ",
        "GET /v2/proj-1/secgroups/sg-1/secGroupRules ",
        "POST /portal/v1/price ",
        "GET /v2/proj-1/secgroups/sg-1 ",
        "GET /v2/proj-1/secgroups/sg-1/secGroupRules ",
        "POST /v2/proj-1/servers ",
        "GET /v2/proj-1/servers/server-1 ",
    ];
    let heads = server.heads();
    assert_eq!(heads.len(), routes.len());
    assert!(
        heads
            .iter()
            .zip(routes)
            .all(|(head, route)| head.starts_with(route.as_bytes()))
    );
}

#[tokio::test]
async fn floating_create_refuses_unrestricted_login_and_honors_read_only() {
    let unsafe_rules = SAFE.replace("192.0.2.8/32", "0.0.0.0/0");
    let server = Server::start(vec![
        response("200 OK", INVENTORY),
        response("200 OK", GROUP),
        response("200 OK", &unsafe_rules),
    ]);
    let (exit, stdout, _) = invoke(&server, &[CREATE, &["--read-only"]].concat(), false).await;
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(server.requests(), 0);
    let (exit, stdout, stderr) = invoke(&server, CREATE, false).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    let refusal: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(refusal["error"]["code"], "OpenLogin");
    assert!(refusal["error"].get("status").is_none());
    assert!(refusal["error"].get("operation").is_none());
    assert_eq!(server.requests(), 3);
}
