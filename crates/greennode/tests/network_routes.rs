use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};

const TOKEN: &str = "toy-secret";

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

struct Case {
    command: &'static str,
    product: Product,
    args: &'static [&'static str],
    requests: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    Case {
        command: "get-route-table",
        product: Product::Vserver,
        args: &["--route-table-id", "rt-2"],
        requests: &[(
            "GET /v2/proj-1/route-table/rt-2 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/get_route_table.json"),
        )],
    },
    Case {
        command: "get-virtual-ip-address",
        product: Product::Vserver,
        args: &["--virtual-ip-address-id", "vip-1"],
        requests: &[(
            "GET /v2/proj-1/virtualIpAddress/vip-1 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/get_virtual_ip_address.json"),
        )],
    },
    Case {
        command: "list-address-pairs-by-virtual-ip-address",
        product: Product::Vserver,
        args: &["--virtual-ip-address-id", "vip-1"],
        requests: &[(
            "GET /v2/proj-1/virtualIpAddress/vip-1/addressPairs ",
            include_str!(
                "../../greennode-sdk/tests/fixtures/network/list_address_pairs_by_virtual_ip_address.json"
            ),
        )],
    },
    Case {
        command: "list-address-pairs-by-virtual-subnet",
        product: Product::Vserver,
        args: &["--virtual-subnet-id", "subnet-1"],
        requests: &[(
            "GET /v2/proj-1/virtual-subnets/subnet-1/addressPairs ",
            include_str!(
                "../../greennode-sdk/tests/fixtures/network/list_virtual_subnet_address_pairs.json"
            ),
        )],
    },
    Case {
        command: "list-all-virtual-ip-address-address-pairs",
        product: Product::Vserver,
        args: &[],
        requests: &[
            (
                "GET /v2/proj-1/virtualIpAddress?name=&page=1&size=10000 ",
                include_str!(
                    "../../greennode-sdk/tests/fixtures/network/list_virtual_ip_addresses.json"
                ),
            ),
            (
                "GET /v2/proj-1/virtualIpAddress/vip-1/addressPairs ",
                include_str!(
                    "../../greennode-sdk/tests/fixtures/network/list_address_pairs_by_virtual_ip_address.json"
                ),
            ),
        ],
    },
    Case {
        command: "list-interconnects",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/interconnects?name=test+%26+sample&page=2&size=5 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_interconnects.json"),
        )],
    },
    Case {
        command: "list-network-interfaces",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/network-interfaces-elastic?name=test+%26+sample&page=2&size=5 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_network_interfaces.json"),
        )],
    },
    Case {
        command: "list-peerings",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/peering?name=test+%26+sample&page=2&size=5 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_peerings.json"),
        )],
    },
    Case {
        command: "list-route-table-routes",
        product: Product::Vserver,
        args: &[],
        requests: &[(
            "GET /v2/proj-1/route-table?name=&page=1&size=10000 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_route_tables.json"),
        )],
    },
    Case {
        command: "list-route-tables",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/route-table?name=test+%26+sample&page=2&size=5 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_route_tables.json"),
        )],
    },
    Case {
        command: "list-virtual-ip-addresses",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/virtualIpAddress?name=test+%26+sample&page=2&size=5 ",
            include_str!(
                "../../greennode-sdk/tests/fixtures/network/list_virtual_ip_addresses.json"
            ),
        )],
    },
    Case {
        command: "list-vpn-connections",
        product: Product::Vnetwork,
        args: &["--page", "2", "--size", "5"],
        requests: &[(
            "GET /vnetwork/v1/proj-1/vpns?params=%7B%22search%22%3A%5B%7B%22field%22%3A%22any%22%2C%22value%22%3A%22%22%7D%5D%2C%22sort%22%3A%7B%7D%2C%22page%22%3A2%2C%22size%22%3A5%7D ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_vpn_connections.json"),
        )],
    },
    Case {
        command: "list-wanips",
        product: Product::Vserver,
        args: &["--name", "test & sample", "--page", "2", "--size", "5"],
        requests: &[(
            "GET /v2/proj-1/wanIps?name=test+%26+sample&page=2&size=5 ",
            include_str!("../../greennode-sdk/tests/fixtures/network/list_wan_ips.json"),
        )],
    },
];

#[test]
fn routing_and_connectivity_match_go_goldens() {
    for case in CASES {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                case.requests
                    .iter()
                    .map(|(_, body)| response("200 OK", body))
                    .collect(),
            );
            let (status, stdout, stderr) =
                invoke(case.product, &server, format, case.command, case.args);
            assert_eq!(status, ExitCode::SUCCESS, "{} {format}", case.command);
            assert!(stderr.is_empty());
            let golden = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
                format!("tests/fixtures/network/{}.{format}.golden", case.command),
            ))
            .unwrap();
            assert_eq!(stdout, golden, "{} {format}", case.command);
            let heads = server.heads();
            assert_eq!(heads.len(), case.requests.len());
            for (head, (request, _)) in heads.iter().zip(case.requests) {
                assert!(head.starts_with(request.as_bytes()), "{}", case.command);
            }
        }
    }
}

#[test]
fn routing_cli_keeps_go_failure_envelopes() {
    for (command, product, args, http_status, body, exit, expected) in [
        (
            "list-wanips",
            Product::Vserver,
            &[][..],
            "200 OK",
            r#"{"listData":4}"#,
            1,
            r#"{"error":{"code":"RequestFailed","message":"network.ListWANIPs: request failed: response failed to decode","operation":"network.ListWANIPs"}}"#,
        ),
        (
            "get-route-table",
            Product::Vserver,
            &["--route-table-id", "rt-1"],
            "404 Not Found",
            r#"{"message":"toy%2Dsecret"}"#,
            4,
            r#"{"error":{"code":"NotFound","message":"security group response withheld","status":404,"operation":"network.GetRouteTable"}}"#,
        ),
        (
            "list-vpn-connections",
            Product::Vnetwork,
            &[],
            "200 OK",
            r#"{"success":false,"code":"withheld"}"#,
            1,
            r#"{"error":{"code":"RequestFailed","message":"VPN request failed; server details withheld","status":200,"operation":"network.ListVPNConnections"}}"#,
        ),
        (
            "list-vpn-connections",
            Product::Vnetwork,
            &["--page", "-1"],
            "200 OK",
            "{}",
            2,
            r#"{"error":{"code":"InvalidUsage","message":"vngcloud: invalid input: network.ListVPNConnections requires nonnegative Page and Size"}}"#,
        ),
    ] {
        let server = Server::start(vec![response(http_status, body)]);
        let (status, stdout, stderr) = invoke(product, &server, "json", command, args);
        assert_eq!(status, ExitCode::from(exit), "{command}");
        assert!(stdout.is_empty());
        assert_eq!(stderr, format!("{expected}\n").as_bytes(), "{command}");
        assert_eq!(server.heads().len(), usize::from(exit != 2));
    }
}
