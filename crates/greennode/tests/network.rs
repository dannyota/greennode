use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};

const TOKEN: &str = "toy-secret";
const NAT_ARGS: &[&str] = &["--nat-id", "nat-1", "--zone-id", "zone-1"];

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

fn invoke(
    product: Product,
    server: &Server,
    format: &str,
    command: &str,
    args: &[&str],
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_invocation(Invocation {
            args: [
                "greennode",
                "--region",
                "hcm-3",
                "--project-id",
                "proj-1",
                "--read-only",
                "--output",
                format,
                "network",
                command,
            ]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
            source: &Source,
            config_options: with_product_loopback(
                product,
                ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
                server,
            ),
            stdin: &mut std::io::empty(),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context: &context,
        }));
    for stream in [&stdout, &stderr] {
        assert!(
            !stream
                .windows(TOKEN.len())
                .any(|part| part == TOKEN.as_bytes())
        );
    }
    (status, stdout, stderr)
}

#[test]
fn network_reads_match_go_goldens() {
    for (command, fixture, product, args, request) in [
        (
            "get-nat-instance",
            include_str!("../../greennode-sdk/tests/fixtures/network/get_nat_instance.json"),
            Product::Vnetwork,
            NAT_ARGS,
            "GET /vnetwork/v1/zone-1/proj-1/nats/nat-1 ",
        ),
        (
            "list-nat-inbound-rules",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_nat_inbound_rules.json"),
            Product::Vnetwork,
            NAT_ARGS,
            "GET /vnetwork/v1/zone-1/proj-1/nats/nat-1/sec-groups/rules ",
        ),
        (
            "list-bandwidth-packages",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_bandwidth_packages.json"),
            Product::Vserver,
            &[],
            "GET /vserver/bandwidth-api/v1/projects/proj-1/bandwidth/package-config ",
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(vec![response("200 OK", fixture)]);
            let (status, stdout, stderr) = invoke(product, &server, format, command, args);
            assert_eq!(status, ExitCode::SUCCESS, "{command} {format}");
            assert!(stderr.is_empty());
            let golden = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/network/{command}.{format}.golden")),
            )
            .unwrap();
            assert_eq!(stdout, golden, "{command} {format}");
            let heads = server.heads();
            assert_eq!(heads.len(), 1);
            assert!(heads[0].starts_with(request.as_bytes()));
        }
    }
}

#[test]
fn nat_required_id_and_invalid_response_keep_distinct_exit_codes() {
    let server = Server::start(vec![response("200 OK", r#"{"success":true,"data":null}"#)]);
    let (status, stdout, _) = invoke(
        Product::Vnetwork,
        &server,
        "json",
        "list-nat-inbound-rules",
        &["--zone-id", "zone-1"],
    );
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty() && server.heads().is_empty());
    let (status, stdout, stderr) = invoke(
        Product::Vnetwork,
        &server,
        "json",
        "list-nat-inbound-rules",
        NAT_ARGS,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    let envelope = &envelope["error"];
    assert_eq!(envelope["code"], "InvalidResponse");
    assert_eq!(envelope["message"], "NAT response withheld");
    assert_eq!(envelope["status"], 200);
    assert_eq!(envelope["operation"], "network.ListNATInboundRules");
}
