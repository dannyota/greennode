#[path = "network/endpoint_errors.rs"]
mod endpoint_errors;
#[path = "network/nat_errors.rs"]
mod nat_errors;

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
    let options = with_product_loopback(
        product,
        greennode_sdk::testing::with_clock(
            ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
            greennode_sdk::testing::retry_clock(),
        ),
        server,
    );
    invoke_with_options(options, format, command, args)
}

fn invoke_with_options(
    options: ConfigOptions,
    format: &str,
    command: &str,
    args: &[&str],
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_in_project(options, "proj-1", format, command, args)
}

fn invoke_in_project(
    options: ConfigOptions,
    project_id: &str,
    format: &str,
    command: &str,
    args: &[&str],
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_in_mode(options, project_id, format, command, args, true)
}

fn invoke_in_mode(
    options: ConfigOptions,
    project_id: &str,
    format: &str,
    command: &str,
    args: &[&str],
    read_only: bool,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_invocation(Invocation {
            args: ["greennode", "--region", "hcm-3", "--project-id", project_id]
                .into_iter()
                .chain(read_only.then_some("--read-only"))
                .chain(["--output", format, "network", command])
                .chain(args.iter().copied())
                .map(OsString::from)
                .collect(),
            source: &Source,
            config_options: options,
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

macro_rules! fixture {
    ($file:literal) => {
        include_str!(concat!(
            "../../greennode-sdk/tests/fixtures/network/",
            $file,
            ".json"
        ))
    };
}

#[test]
fn network_inventory_reads_match_go_goldens() {
    type Case = (
        &'static str,
        &'static str,
        Product,
        &'static [&'static str],
        &'static str,
        &'static [&'static str],
    );
    let cases: &[Case] = &[
        (
            "get-endpoint",
            fixture!("get_endpoint"),
            Product::Vnetwork,
            &["--endpoint-id", "endpoint-1"],
            "GET /vnetwork/v1/zone-a/proj-1/endpoints/endpoint-1 ",
            &[fixture!("list_vnetwork_regions")],
        ),
        (
            "list-endpoint-tags",
            fixture!("list_endpoint_tags"),
            Product::Vnetwork,
            &["--endpoint-id", "endpoint-1"],
            "GET /vnetwork/v1/proj-1/tags?resourceUuid=endpoint-1 ",
            &[],
        ),
        (
            "list-endpoints",
            fixture!("list_endpoints"),
            Product::Vnetwork,
            &[
                "--zone-id",
                "zone-a",
                "--vpc-id",
                "vpc-1",
                "--uuid",
                "endpoint-1",
                "--page",
                "2",
                "--size",
                "25",
            ],
            "GET /vnetwork/v1/zone-a/proj-1/endpoints?params=",
            &[],
        ),
        (
            "list-nat-instances",
            fixture!("list_nat_instances"),
            Product::Vnetwork,
            &["--zone-id", "zone-1", "--page", "2", "--size", "5"],
            "GET /vnetwork/v1/zone-1/proj-1/nats?params=",
            &[],
        ),
        (
            "list-nat-zones",
            fixture!("list_nat_zones"),
            Product::Vnetwork,
            &["--zone-id", "zone-1"],
            "GET /vnetwork/v1/zone-1/proj-1/nats/zones?params=",
            &[],
        ),
        (
            "list-nat-packages",
            fixture!("list_nat_packages"),
            Product::Vnetwork,
            &["--zone-id", "zone-1", "--availability-zone-id", "az-1"],
            "GET /vnetwork/v1/zone-1/proj-1/nats/nat-package?params=",
            &[],
        ),
        (
            "list-v-network-regions",
            fixture!("list_vnetwork_regions"),
            Product::Vnetwork,
            &[],
            "GET /vnetwork/v1/regions ",
            &[],
        ),
        (
            "list-servers-by-security-group",
            fixture!("list_servers_by_security_group"),
            Product::Vserver,
            &["--security-group-id", "group-1"],
            "GET /v2/proj-1/secgroups/group-1/servers ",
            &[],
        ),
        (
            "list-servers-by-subnet",
            fixture!("list_servers_by_subnet"),
            Product::Vserver,
            &["--subnet-id", "subnet-1"],
            "GET /v2/proj-1/servers/subnets/subnet-1 ",
            &[],
        ),
    ];
    for &(command, fixture, product, args, request, preceding) in cases {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                preceding
                    .iter()
                    .copied()
                    .chain([fixture])
                    .map(|body| response("200 OK", body))
                    .collect(),
            );
            let project = Server::start(vec![response(
                "200 OK",
                r#"{"projects":[{"projectId":"proj-1","region":"hcm-3","userId":123}]}"#,
            )]);
            let mut options = with_product_loopback(
                product,
                ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
                &server,
            );
            if command == "list-endpoint-tags" {
                options = with_product_loopback(Product::Vserver, options, &project);
            }
            let (status, stdout, stderr) = invoke_with_options(options, format, command, args);
            assert_eq!(
                status,
                ExitCode::SUCCESS,
                "{command} {format} {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(stderr.is_empty());
            let golden = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/network/{command}.{format}.golden")),
            )
            .unwrap();
            assert!(
                stdout == golden,
                "{command} {format} output differs from the Go golden"
            );
            let heads = server.heads();
            assert_eq!(heads.len(), preceding.len() + 1);
            assert!(heads[preceding.len()].starts_with(request.as_bytes()));
            if command == "list-endpoint-tags" {
                assert!(project.heads()[0].starts_with(b"GET /v1/projects "));
            }
        }
    }
}

#[test]
fn network_prices_match_go_goldens_and_write_coverage_notes() {
    let body =
        include_str!("../../greennode-sdk/tests/fixtures/pricing/discovery_quote_public_vm.json");
    for format in ["json", "table", "text"] {
        let server = Server::start((0..3).map(|_| response("200 OK", body)).collect());
        let (status, stdout, stderr) =
            invoke(Product::Portal, &server, format, "list-network-prices", &[]);
        assert_eq!(status, ExitCode::SUCCESS);
        let mut stdout = String::from_utf8(stdout).unwrap();
        if format == "json" {
            let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
            let quoted_at = output["Metadata"]["QuotedAt"].as_str().unwrap();
            stdout = stdout.replacen(quoted_at, "<time>", 1);
        }
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/network");
        let golden =
            std::fs::read_to_string(directory.join(format!("list-network-prices.{format}.golden")))
                .unwrap();
        let errors =
            std::fs::read(directory.join(format!("list-network-prices.{format}.stderr.golden")))
                .unwrap();
        assert_eq!(stdout, golden, "network prices {format}");
        assert_eq!(stderr, errors);
        assert_eq!(server.heads().len(), 3);
    }
}

#[test]
fn network_purchase_quotes_match_go_goldens_on_read_only_profiles() {
    const PROJECT: &str = "pro-00000000-0000-0000-0000-000000000001";
    const ENDPOINT_ARGS: &[&str] = &[
        "--name",
        "edge",
        "--vpc-id",
        "net-00000000-0000-0000-0000-000000000002",
        "--subnet-id",
        "sub-00000000-0000-0000-0000-000000000003",
    ];
    const NAT_ARGS: &[&str] = &[
        "--name",
        "edge",
        "--zone-id",
        "zone-1",
        "--availability-zone-id",
        "az-1",
        "--package-id",
        "package-1",
        "--vpc-id",
        "vpc-1",
    ];
    for (command, project_id, args, fixtures) in [
        (
            "quote-create-nat-instance",
            "proj-1",
            NAT_ARGS,
            &[
                fixture!("nat_quote_whitelist"),
                fixture!("list_nat_zones"),
                fixture!("list_nat_packages"),
                fixture!("nat_quote_vpcs"),
                fixture!("quote_create_nat"),
            ][..],
        ),
        (
            "quote-create-endpoint",
            PROJECT,
            ENDPOINT_ARGS,
            &[
                fixture!("endpoint_quote_regions"),
                fixture!("endpoint_quote_zones"),
                fixture!("endpoint_quote_categories"),
                fixture!("endpoint_quote_services"),
                fixture!("endpoint_quote_packages"),
                fixture!("endpoint_quote_vpcs"),
                fixture!("endpoint_quote_subnets"),
                fixture!("quote_create_nat"),
            ][..],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let endpoint = Server::start(
                fixtures
                    .iter()
                    .map(|body| response("200 OK", body))
                    .collect(),
            );
            let project = Server::start(vec![response(
                "200 OK",
                fixture!("endpoint_quote_projects"),
            )]);
            let mut options = with_product_loopback(
                Product::Vnetwork,
                ConfigOptions::default().with_static_token(Secret::new(TOKEN)),
                &endpoint,
            );
            if command == "quote-create-endpoint" {
                options = with_product_loopback(Product::Vserver, options, &project);
            }
            let (status, stdout, stderr) =
                invoke_in_project(options, project_id, format, command, args);
            assert_eq!(status, ExitCode::SUCCESS, "{command} {format}");
            assert!(stderr.is_empty());
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/network/{command}.{format}.golden"));
            assert!(
                stdout == std::fs::read(path).unwrap(),
                "{command} {format} output differs from the Go golden"
            );
            let heads = endpoint.heads();
            assert_eq!(heads.len(), fixtures.len());
            assert!(
                heads
                    .last()
                    .unwrap()
                    .starts_with(b"POST /vnetwork/billing/v1/")
            );
            assert_eq!(
                project.heads().len(),
                usize::from(command == "quote-create-endpoint")
            );
        }
    }
}

#[path = "network/vpc_writes.rs"]
mod vpc_writes;
