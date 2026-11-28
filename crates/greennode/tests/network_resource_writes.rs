use std::{
    ffi::{OsStr, OsString},
    process::ExitCode,
};

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_write_loopback};
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
    invoke_in_project(
        options(Product::Vserver, server),
        "proj-1",
        format,
        command,
        args,
    )
}

fn options(product: Product, server: &Server) -> ConfigOptions {
    with_write_loopback(
        product,
        ConfigOptions::default().with_static_token(Secret::new("toy-credential")),
        server,
    )
}

fn invoke_in_project(
    config_options: ConfigOptions,
    project_id: &str,
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
                project_id,
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
            config_options,
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
fn world_open_rule_requires_consent_before_network() {
    let server = Server::start(vec![]);
    let (exit, stdout, stderr) = invoke(
        &server,
        "json",
        "create-security-group-rule",
        &[
            "--cli-input-json",
            r#"{"SecurityGroupID":"sg-1","Direction":"ingress","Protocol":"icmp","RemoteIPPrefix":"::/0"}"#,
        ],
    );
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("pass --yes"));
    assert!(server.heads().is_empty());
}

#[test]
fn endpoint_order_requires_consent_before_network() {
    let server = Server::start(vec![]);
    let (exit, stdout, stderr) = invoke(
        &server,
        "json",
        "create-endpoint",
        &[
            "--name",
            "endpoint",
            "--vpc-id",
            "net-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "--subnet-id",
            "sub-bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "--max-price",
            "0",
        ],
    );
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("buys a paid endpoint; pass --yes"));
    assert!(server.heads().is_empty());
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../greennode-sdk/tests/fixtures/network/",
            $name,
            ".json"
        ))
    };
}

const GROUP: &str = fixture!("get_security_group");
const VIP: &str = fixture!("create_virtual_ip_address");
const UPDATED_VIP: &str = fixture!("update_virtual_ip_address");
const EMPTY: &str = r#"{"success":true,"page":1,"size":1000,"totalPage":0,"total":0,"data":[]}"#;
const ACK: &str = r#"{"success":true,"code":0}"#;
const PROJECT: &str = "pro-00000000-0000-0000-0000-000000000001";
const ENDPOINT: &str = r#"{"success":true,"page":1,"size":1000,"totalPage":1,"total":1,"data":[{"uuid":"enp-00000000-0000-0000-0000-0000000000c1","endpointName":"demo","endpointServiceId":"66b500000000000000000009","vpcId":"net-00000000-0000-0000-0000-000000000002","status":"ACTIVE","endpointType":"Gateway","zoneUuid":"HCM03-1C","enableDnsName":false,"service":{"uuid":"00000000-0000-0000-0000-0000000000a1","name":"vstorage.hcm03"},"subnet":{"uuid":"sub-00000000-0000-0000-0000-000000000003"},"packageId":{"uuid":"00000000-0000-0000-0000-0000000000b1"},"project":{"VServerProjectId":"pro-00000000-0000-0000-0000-000000000001"},"endpointResource":null,"endpointDomains":[]}]}"#;

type Step = (&'static str, &'static str, &'static str);

fn server(steps: &[Step]) -> Server {
    Server::start(
        steps
            .iter()
            .map(|(_, status, body)| response(status, body))
            .collect(),
    )
}

fn assert_routes(server: &Server, steps: &[Step]) {
    let heads = server.heads();
    assert_eq!(heads.len(), steps.len());
    for (head, (route, _, _)) in heads.iter().zip(steps) {
        assert!(head.starts_with(route.as_bytes()), "expected route {route}");
    }
}

fn assert_golden(result: (ExitCode, Vec<u8>, Vec<u8>), command: &str, format: &str) {
    let (status, stdout, stderr) = result;
    assert_eq!(
        status,
        ExitCode::SUCCESS,
        "{command} {format}: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/network/{command}.{format}.golden"));
    assert!(
        stdout == std::fs::read(path).unwrap(),
        "{command} {format} output differs from the Go golden"
    );
}

#[test]
fn vserver_resource_writes_match_go_goldens() {
    let cases: &[(&str, &str, &[Step])] = &[
        (
            "create-security-group",
            r#"{"Name":"example"}"#,
            &[
                (
                    "POST /v2/proj-1/secgroups ",
                    "201 Created",
                    fixture!("create_security_group"),
                ),
                ("GET /v2/proj-1/secgroups/secg-1 ", "200 OK", GROUP),
            ],
        ),
        (
            "update-security-group",
            r#"{"SecurityGroupID":"secg-1","Name":"example"}"#,
            &[
                ("GET /v2/proj-1/secgroups/secg-1 ", "200 OK", GROUP),
                ("PUT /v2/proj-1/secgroups/secg-1 ", "200 OK", ""),
                ("GET /v2/proj-1/secgroups/secg-1 ", "200 OK", GROUP),
            ],
        ),
        (
            "delete-security-group",
            r#"{"SecurityGroupID":"secg-1"}"#,
            &[
                ("GET /v2/proj-1/secgroups/secg-1 ", "200 OK", GROUP),
                (
                    "GET /v2/proj-1/secgroups/secg-1/servers ",
                    "200 OK",
                    r#"{"data":[]}"#,
                ),
                ("DELETE /v2/proj-1/secgroups/secg-1 ", "204 No Content", ""),
            ],
        ),
        (
            "create-security-group-rule",
            r#"{"SecurityGroupID":"secg-1","Direction":"ingress","Protocol":"tcp","RemoteIPPrefix":"203.0.113.0/24","PortRangeMin":22,"Description":"example"}"#,
            &[(
                "POST /v2/proj-1/secgroups/secg-1/secgroupRules ",
                "201 Created",
                fixture!("create_security_group_rule"),
            )],
        ),
        (
            "delete-security-group-rule",
            r#"{"SecurityGroupID":"secg-1","SecurityGroupRuleID":"secr-1"}"#,
            &[
                (
                    "GET /v2/proj-1/secgroups/secg-1/secGroupRules ",
                    "200 OK",
                    fixture!("list_security_group_rules"),
                ),
                (
                    "DELETE /v2/proj-1/secgroups/secg-1/secgroupRules/secr-1 ",
                    "204 No Content",
                    "",
                ),
            ],
        ),
        (
            "create-virtual-ip-address",
            r#"{"SubnetID":"subnet-1","Name":"example","Mode":"Active/Passive"}"#,
            &[("POST /v2/proj-1/virtualIpAddress ", "201 Created", VIP)],
        ),
        (
            "update-virtual-ip-address",
            r#"{"VirtualIPAddressID":"vip-1","Mode":"Active/Active"}"#,
            &[
                ("GET /v2/proj-1/virtualIpAddress/vip-1 ", "200 OK", VIP),
                (
                    "PUT /v2/proj-1/virtualIpAddress/vip-1 ",
                    "200 OK",
                    UPDATED_VIP,
                ),
                (
                    "GET /v2/proj-1/virtualIpAddress/vip-1 ",
                    "200 OK",
                    UPDATED_VIP,
                ),
            ],
        ),
        (
            "delete-virtual-ip-address",
            r#"{"VirtualIPAddressID":"vip-1"}"#,
            &[
                ("GET /v2/proj-1/virtualIpAddress/vip-1 ", "200 OK", VIP),
                (
                    "GET /v2/proj-1/virtualIpAddress/vip-1/addressPairs ",
                    "200 OK",
                    r#"{"data":[]}"#,
                ),
                (
                    "DELETE /v2/proj-1/virtualIpAddress/vip-1 ",
                    "204 No Content",
                    "",
                ),
            ],
        ),
    ];
    for (command, input, steps) in cases {
        for format in ["json", "table", "text"] {
            let server = server(steps);
            assert_golden(
                invoke(
                    &server,
                    format,
                    command,
                    &["--yes", "--cli-input-json", input],
                ),
                command,
                format,
            );
            assert_routes(&server, steps);
        }
    }
}

#[test]
fn endpoint_writes_match_go_goldens() {
    const PATH: &str =
        "/vnetwork/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/endpoints";
    let create = [
        (
            "GET /vnetwork/v1/regions ",
            "200 OK",
            fixture!("endpoint_quote_regions"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_zones"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_categories"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_services"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_packages"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_vpcs"),
        ),
        (
            "GET /vnetwork/v1/",
            "200 OK",
            fixture!("endpoint_quote_subnets"),
        ),
        ("GET /vnetwork/v1/", "200 OK", EMPTY),
        (
            "POST /vnetwork/billing/v1/",
            "200 OK",
            fixture!("quote_create_nat"),
        ),
        (
            "POST /vnetwork/v1/",
            "201 Created",
            r#"{"success":true,"code":0,"data":{"uuid":"enp-00000000-0000-0000-0000-0000000000c1","name":"demo"}}"#,
        ),
        ("GET /vnetwork/v1/", "200 OK", ENDPOINT),
    ];
    let delete = [
        (
            "GET /vnetwork/v1/regions ",
            "200 OK",
            fixture!("endpoint_quote_regions"),
        ),
        ("GET /vnetwork/v1/", "200 OK", ENDPOINT),
        ("DELETE /vnetwork/v1/", "200 OK", ACK),
        ("GET /vnetwork/v1/", "200 OK", EMPTY),
    ];
    for (command, input, steps, write_index) in [
        (
            "create-endpoint",
            r#"{"Name":"demo","VPCID":"net-00000000-0000-0000-0000-000000000002","SubnetID":"sub-00000000-0000-0000-0000-000000000003","MaxPrice":712400}"#,
            create.as_slice(),
            9,
        ),
        (
            "delete-endpoint",
            r#"{"EndpointID":"enp-00000000-0000-0000-0000-0000000000c1","VPCID":"net-00000000-0000-0000-0000-000000000002"}"#,
            delete.as_slice(),
            2,
        ),
    ] {
        for format in ["json", "table", "text"] {
            let vserver = Server::start(vec![response(
                "200 OK",
                fixture!("endpoint_quote_projects"),
            )]);
            let vnetwork = server(steps);
            let options = with_write_loopback(
                Product::Vserver,
                options(Product::Vnetwork, &vnetwork),
                &vserver,
            );
            assert_golden(
                invoke_in_project(
                    options,
                    PROJECT,
                    format,
                    command,
                    &["--yes", "--cli-input-json", input],
                ),
                command,
                format,
            );
            assert_eq!(vserver.heads().len(), 1);
            assert_routes(&vnetwork, steps);
            let heads = vnetwork.heads();
            let expected = if command == "create-endpoint" {
                format!("POST {PATH} ")
            } else {
                format!("DELETE {PATH}/enp-00000000-0000-0000-0000-0000000000c1 ")
            };
            assert!(heads[write_index].starts_with(expected.as_bytes()));
        }
    }
}

#[test]
fn nat_delete_matches_go_goldens() {
    let steps = [
        (
            "GET /vnetwork/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/nats/vpcs?",
            "200 OK",
            fixture!("nat_vpc_picker"),
        ),
        (
            "GET /vnetwork/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/nats?",
            "200 OK",
            fixture!("nat_write_inventory"),
        ),
        (
            "DELETE /vnetwork/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/nats/nat-1 ",
            "200 OK",
            ACK,
        ),
        (
            "GET /vnetwork/v1/66b500000000000000000001/pro-00000000-0000-0000-0000-000000000001/nats?",
            "200 OK",
            EMPTY,
        ),
    ];
    for format in ["json", "table", "text"] {
        let server = server(&steps);
        let command = "delete-nat-instance";
        let input = r#"{"ZoneID":"66b500000000000000000001","VPCID":"vpc-1","NATID":"nat-1"}"#;
        assert_golden(
            invoke_in_project(
                options(Product::Vnetwork, &server),
                PROJECT,
                format,
                command,
                &["--yes", "--cli-input-json", input],
            ),
            command,
            format,
        );
        assert_routes(&server, &steps);
    }
}

#[test]
fn nat_read_only_refusals_use_invalid_usage() {
    for (command, args, code) in [
        (
            "delete-nat-instance",
            &[
                "--zone-id",
                "zone-1",
                "--vpc-id",
                "vpc-1",
                "--nat-id",
                "nat-1",
            ][..],
            "InvalidUsage",
        ),
        (
            "delete-security-group",
            &["--security-group-id", "sg-1"][..],
            "ReadOnly",
        ),
    ] {
        let server = Server::start(vec![]);
        let args: Vec<_> = ["--read-only", "--yes"]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        let (status, stdout, stderr) = invoke(&server, "json", command, &args);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&stderr).unwrap(),
            serde_json::json!({"error":{"code":code,"message":"read-only: refused by --read-only"}})
        );
        assert!(server.heads().is_empty());
    }
}
