use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};

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
    server: &Server,
    format: &str,
    command: &str,
    args: &[&str],
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = tokio::runtime::Builder::new_current_thread()
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
                Product::Vserver,
                ConfigOptions::default().with_static_token(Secret::new("toy-credential")),
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
    (code, stdout, stderr)
}

#[test]
fn vpc_inventory_matches_go_goldens() {
    type Case = (
        &'static str,
        &'static [&'static str],
        &'static [(&'static str, &'static str)],
    );
    let cases: &[Case] = &[
        (
            "get-dhcp-options",
            &["--dhcp-options-id", "dop-1"],
            &[(
                "GET /v2/proj-1/dhcp_option/dop-1 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/get_dhcp_options.json"),
            )],
        ),
        (
            "get-network-acl",
            &["--network-acl-id", "acl-1"],
            &[(
                "GET /v2/proj-1/network-acl/acl-1 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/get_network_acl.json"),
            )],
        ),
        (
            "get-security-group",
            &["--security-group-id", "sg-1"],
            &[(
                "GET /v2/proj-1/secgroups/sg-1 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/get_security_group.json"),
            )],
        ),
        (
            "get-subnet",
            &["--vpc-id", "vpc-1", "--subnet-id", "subnet-1"],
            &[(
                "GET /v2/proj-1/networks/vpc-1/subnets/subnet-1 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/get_subnet.json"),
            )],
        ),
        (
            "get-vpc",
            &["--vpc-id", "vpc-1"],
            &[(
                "GET /v2/proj-1/networks/vpc-1 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/get_vpc.json"),
            )],
        ),
        (
            "list-all-security-group-rules",
            &[],
            &[
                (
                    "GET /v2/proj-1/secgroups?name=&page=1&size=10000 ",
                    include_str!(
                        "../../greennode-sdk/tests/fixtures/network/list_security_groups.json"
                    ),
                ),
                (
                    "GET /v2/proj-1/secgroups/sg-1/secGroupRules ",
                    include_str!(
                        "../../greennode-sdk/tests/fixtures/network/list_security_group_rules.json"
                    ),
                ),
            ],
        ),
        (
            "list-dhcp-options",
            &[],
            &[(
                "GET /v2/proj-1/dhcp_option?name=&page=1&size=10000 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/list_dhcp_options.json"),
            )],
        ),
        (
            "list-network-acls",
            &[],
            &[(
                "GET /v2/proj-1/network-acl/list?name=&page=1&size=10000 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/list_network_acls.json"),
            )],
        ),
        (
            "list-security-group-rules",
            &["--security-group-id", "sg-1"],
            &[(
                "GET /v2/proj-1/secgroups/sg-1/secGroupRules ",
                include_str!(
                    "../../greennode-sdk/tests/fixtures/network/list_security_group_rules.json"
                ),
            )],
        ),
        (
            "list-security-groups",
            &[],
            &[(
                "GET /v2/proj-1/secgroups?name=&page=1&size=10000 ",
                include_str!(
                    "../../greennode-sdk/tests/fixtures/network/list_security_groups.json"
                ),
            )],
        ),
        (
            "list-subnets",
            &[],
            &[
                (
                    "GET /v2/proj-1/networks?name=&page=1&size=10000 ",
                    include_str!("../../greennode-sdk/tests/fixtures/network/list_vpcs.json"),
                ),
                (
                    "GET /v2/proj-1/networks/vpc-1/subnets ",
                    include_str!(
                        "../../greennode-sdk/tests/fixtures/network/list_subnets_by_vpc.json"
                    ),
                ),
            ],
        ),
        (
            "list-subnets-by-vpc",
            &["--vpc-id", "vpc-1"],
            &[(
                "GET /v2/proj-1/networks/vpc-1/subnets ",
                include_str!("../../greennode-sdk/tests/fixtures/network/list_subnets_by_vpc.json"),
            )],
        ),
        (
            "list-vpcs",
            &[],
            &[(
                "GET /v2/proj-1/networks?name=&page=1&size=10000 ",
                include_str!("../../greennode-sdk/tests/fixtures/network/list_vpcs.json"),
            )],
        ),
    ];
    for &(command, args, reads) in cases {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                reads
                    .iter()
                    .map(|(_, fixture)| response("200 OK", fixture))
                    .collect(),
            );
            let (code, stdout, stderr) = invoke(&server, format, command, args);
            assert_eq!(code, ExitCode::SUCCESS, "{command} {format}");
            assert!(stderr.is_empty());
            let golden = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/network/{command}.{format}.golden")),
            )
            .unwrap();
            assert_eq!(stdout, golden, "{command} {format}");
            let heads = server.heads();
            assert_eq!(heads.len(), reads.len());
            for (head, (request, _)) in heads.iter().zip(reads) {
                assert!(head.starts_with(request.as_bytes()));
            }
        }
    }
}

#[test]
fn inventory_missing_required_id_fails_before_sending() {
    let server = Server::start(Vec::new());
    let (code, stdout, _) = invoke(&server, "json", "get-subnet", &["--vpc-id", "vpc-1"]);
    assert_eq!(code, ExitCode::from(2));
    assert!(stdout.is_empty() && server.heads().is_empty());
}
