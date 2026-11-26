use super::*;

const CERTIFICATE_JSON: &str =
    r#"{"Certificate":"-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n"}"#;
const CERTIFICATE_REPLY: &str =
    r#"{"data":{"uuid":"cert-1","name":"toy-ca","certificateType":"CA"}}"#;
const CURRENT: &str =
    r#"{"data":{"uuid":"lb-1","packageId":"package-1","progressStatus":"CREATED"}}"#;

#[tokio::test]
async fn write_output_matches_go_in_all_formats() {
    for format in ["json", "table", "text"] {
        let scenarios = [
            (
                "import-certificate",
                vec![
                    "--name",
                    "toy-ca",
                    "--type",
                    "CA",
                    "--cli-input-json",
                    CERTIFICATE_JSON,
                ],
                vec![response("201 Created", CERTIFICATE_REPLY)],
                vec![],
                vec!["POST /v2/proj-1/cas "],
            ),
            (
                "create-load-balancer",
                vec![
                    "--name",
                    "toy-lb",
                    "--type",
                    "Layer 4",
                    "--scheme",
                    "Internal",
                    "--subnet-id",
                    "subnet-1",
                    "--zone-id",
                    "zone-1",
                    "--package-id",
                    "package-1",
                    "--max-price",
                    "9",
                    "--no-wait",
                ],
                vec![response("200 OK", r#"{"uuid":"lb-1"}"#)],
                vec![response("200 OK", r#"{"optimumPrice":9}"#)],
                vec!["POST /v2/proj-1/loadBalancers "],
            ),
            (
                "resize-load-balancer",
                vec![
                    "--load-balancer-id",
                    "lb-1",
                    "--package-id",
                    "package-2",
                    "--no-wait",
                ],
                vec![
                    response("200 OK", CURRENT),
                    response("200 OK", CURRENT),
                    response("202 Accepted", ""),
                ],
                vec![response("200 OK", r#"{"optimumPrice":-3}"#)],
                vec![
                    "GET /v2/proj-1/loadBalancers/lb-1 ",
                    "GET /v2/proj-1/loadBalancers/lb-1 ",
                    "PUT /v2/proj-1/loadBalancers/lb-1/resize ",
                ],
            ),
        ];
        for (command, flags, regional_replies, portal_replies, routes) in scenarios {
            let expected_quotes = portal_replies.len();
            let regional = Server::start(regional_replies);
            let portal = Server::start(portal_replies);
            let options = with_product_loopback(Product::Portal, options(&regional), &portal);
            let mut args = vec![command, "--project-id", "proj-1", "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) =
                invoke_options(&args, false, options, &mut std::io::empty()).await;
            assert_eq!(
                status,
                ExitCode::SUCCESS,
                "{command}/{format}: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(stderr.is_empty());
            let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "tests/fixtures/loadbalancer/{command}.{format}.golden"
            ));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
            assert_eq!(regional.requests(), routes.len());
            assert!(
                regional
                    .heads()
                    .iter()
                    .zip(routes)
                    .all(|(head, route)| head.starts_with(route.as_bytes()))
            );
            assert_eq!(portal.requests(), expected_quotes);
            assert!(
                portal
                    .heads()
                    .iter()
                    .all(|head| head.starts_with(b"POST /v1/price "))
            );
        }
    }
}

#[tokio::test]
async fn tls_import_accepts_pem_stdin_and_withholds_the_key_from_debug_and_results() {
    let key = b"-----BEGIN PRIVATE KEY-----\r\ndG95LWtleQ==\r\n-----END PRIVATE KEY-----\r\n";
    let server = Server::start(vec![response(
        "201 Created",
        r#"{"data":{"uuid":"cert-1","name":"toy-tls","subject":"dG95LWtleQ=="}}"#,
    )]);
    let args = [
        "import-certificate",
        "--project-id",
        "proj-1",
        "--debug",
        "--name",
        "toy-tls",
        "--type",
        "TLS/SSL",
        "--cli-input-json",
        CERTIFICATE_JSON,
        "--private-key-file",
        "-",
    ];
    let (status, stdout, stderr) =
        invoke_options(&args, false, options(&server), &mut key.as_slice()).await;
    assert_eq!(status, ExitCode::SUCCESS);
    let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(output["Certificate"]["Subject"], "<redacted>");
    for bytes in [stdout, stderr] {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("PRIVATE KEY") && !text.contains("dG95LWtleQ=="));
    }
    assert_eq!(server.requests(), 1);
    let submitted: serde_json::Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
    assert!(
        submitted["privateKey"]
            .as_str()
            .is_some_and(|text| text.as_bytes() == key)
    );
}

fn options(server: &Server) -> ConfigOptions {
    with_product_loopback(
        Product::Vlb,
        ConfigOptions::default().with_static_token(Secret::new(BEARER)),
        server,
    )
}
