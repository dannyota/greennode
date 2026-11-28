use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "synthetic-cli-volume-canary";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "hcm-3".into())
    }
}

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn invoke(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_mode(args, server, true)
}

fn invoke_mode(args: &[&str], server: &Server, read_only: bool) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_with_bearer(args, server, read_only, BEARER)
}

fn invoke_with_bearer(
    args: &[&str],
    server: &Server,
    read_only: bool,
    bearer: &str,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_billing_loopback(
        greennode_sdk::testing::with_clock(
            ConfigOptions::default().with_static_token(Secret::new(bearer)),
            greennode_sdk::testing::retry_clock(),
        )
        .with_endpoint_overrides(EndpointOverrides {
            vserver: Some(format!("{}/", server.origin)),
            portal: Some(format!("{}/", server.origin)),
            vserver_backup: Some(format!("{}/vserver/vbackup-gateway/", server.origin)),
            ..Default::default()
        }),
        server,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: std::iter::once("greennode")
            .chain(["--project-id", "proj-1"])
            .chain(read_only.then_some("--read-only"))
            .chain(["volume"])
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options,
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    for stream in [&stdout, &stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains(BEARER) && !text.contains("SECRET-MARKER"));
    }
    (status, stdout, stderr)
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../greennode-sdk/tests/fixtures/volume/",
            $name,
            ".json"
        ))
    };
}

const VOLUME_PAGE: &str =
    r#"{"listData":[{"uuid":"volume-1"}],"page":1,"pageSize":10000,"totalPage":1,"totalItem":1}"#;
const SNAPSHOT_PAGE: &str = r#"{"items":[{"id":"snapshot-1","volumeId":"volume-1"}],"page":1,"pageSize":10000,"totalPages":1,"totalItems":1}"#;
const ZONES: &str = r#"{"data":[{"uuid":"zone-1","name":"Disabled","isEnabled":false}]}"#;
const PARENTS: &str = r#"{"volumeTypeZones":[{"id":"parent-1"}]}"#;
const QUOTE: &str = fixture!("discovery_quote_plain");

fn run_goldens(command: &str, flags: &[&str], responses: &[&str], expected: [&str; 3]) {
    for (format, golden) in ["json", "table", "text"].into_iter().zip(expected) {
        let server = Server::start(
            responses
                .iter()
                .map(|body| response("200 OK", body))
                .collect(),
        );
        let args: Vec<_> = [command, "--output", format]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();
        let (status, stdout, stderr) = invoke(&args, &server);
        assert_eq!(
            status,
            ExitCode::SUCCESS,
            "{command}/{format} {}",
            String::from_utf8_lossy(&stderr)
        );
        let mut stdout = String::from_utf8(stdout).unwrap();
        if command == "list-volume-prices" {
            if format == "json" {
                let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
                let time = parsed["Metadata"]["QuotedAt"].as_str().unwrap();
                assert!(time.ends_with('Z'));
                stdout = stdout.replacen(time, "1970-01-01T00:00:03Z", 1);
                assert!(stderr.is_empty());
            } else {
                assert_eq!(
                    stderr,
                    include_bytes!("fixtures/volume/list-volume-prices.table.golden.stderr")
                );
            }
        } else {
            assert!(stderr.is_empty(), "{command}/{format}");
        }
        assert_eq!(stdout, golden, "{command}/{format}");
        assert_eq!(server.requests(), responses.len(), "{command}/{format}");
    }
}

macro_rules! goldens {
    ($command:literal, $flags:expr, $responses:expr) => {
        run_goldens(
            $command,
            $flags,
            $responses,
            [
                include_str!(concat!("fixtures/volume/", $command, ".json.golden")),
                include_str!(concat!("fixtures/volume/", $command, ".table.golden")),
                include_str!(concat!("fixtures/volume/", $command, ".text.golden")),
            ],
        );
    };
}

#[test]
fn inventory_output_matches_go() {
    goldens!("list-volumes", &[], &[fixture!("list_volumes")]);
    goldens!(
        "get-volume",
        &["--volume-id", "volume-1"],
        &[fixture!("get_volume")]
    );
    goldens!(
        "get-underlying-volume",
        &["--volume-id", "volume-1"],
        &[fixture!("get_underlying_volume")]
    );
    goldens!(
        "list-volumes-by-server",
        &["--server-id", "server-1"],
        &[fixture!("list_volumes_by_server")]
    );
    goldens!(
        "list-volume-type-zones",
        &[],
        &[fixture!("list_volume_type_zones")]
    );
    goldens!("list-volume-types", &[], &[fixture!("list_volume_types")]);
    goldens!(
        "get-volume-type",
        &["--volume-type-id", "type-1"],
        &[fixture!("get_volume_type")]
    );
    goldens!(
        "get-default-volume-type",
        &["--zone-id", "zone-1"],
        &[fixture!("get_default_volume_type")]
    );
    goldens!(
        "list-encryption-types",
        &[],
        &[fixture!("list_encryption_types_keys")]
    );
}

#[test]
fn snapshot_output_matches_go() {
    goldens!(
        "list-snapshots",
        &["--volume-id", "volume-1"],
        &[fixture!("list_snapshots")]
    );
    goldens!("list-all-snapshots", &[], &[VOLUME_PAGE, SNAPSHOT_PAGE]);
    goldens!(
        "list-snapshot-backends",
        &["--name", "HCM-03"],
        &[fixture!("snapshot_backends")]
    );
    goldens!(
        "list-snapshot-policies",
        &["--backend-id", "backend-1"],
        &[fixture!("snapshot_policies")]
    );
}

#[test]
fn quote_output_matches_go() {
    goldens!(
        "quote-create-volume",
        &[
            "--zone-id",
            "zone-1",
            "--size",
            "1",
            "--volume-type-id",
            "type-1"
        ],
        &[QUOTE]
    );
    goldens!(
        "quote-resize-volume",
        &["--volume-id", "volume-1", "--size", "10000"],
        &[fixture!("get_volume"), QUOTE]
    );
    goldens!(
        "list-volume-prices",
        &[
            "--zone-id",
            "zone-1",
            "--encryption",
            "none",
            "--iops",
            "3000"
        ],
        &[ZONES, PARENTS, "[]", fixture!("discovery_types"), QUOTE]
    );
}

#[test]
fn price_discovery_partial_honors_query() {
    let server = Server::start(vec![
        response("200 OK", ZONES),
        response("200 OK", PARENTS),
        response("200 OK", "[]"),
        response("200 OK", fixture!("discovery_types")),
        response(
            "200 OK",
            r#"{"optimumPrice":1,"propertiesPrice":[{"name":"synthetic%2dcli%2dvolume%2dcanary"}]}"#,
        ),
    ]);
    let (status, stdout, stderr) = invoke(
        &[
            "list-volume-prices",
            "--zone-id",
            "zone-1",
            "--iops",
            "3000",
            "--encryption",
            "none",
            "--allow-partial",
            "--query",
            "Items[].Price.Status",
        ],
        &server,
    );
    assert_eq!(status, ExitCode::from(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stdout).unwrap(),
        serde_json::json!(["failed"])
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stderr).unwrap()["error"]["code"],
        "DiscoveryIncomplete"
    );
    assert_eq!(server.requests(), 5);
}

#[test]
fn invalid_sort_and_missing_volume_id_stop_before_requests() {
    for args in [
        &["list-volume-prices", "--sort-by", "ram"][..],
        &["get-volume"][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, _) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert_eq!(server.requests(), 0);
    }
}

#[test]
fn integral_dynamic_volume_numbers_use_the_go_float_renderer() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"data":{"uuid":"volume-1","status":"AVAILABLE","throughPut":1.0,"volumeType":{"nested":[2.0]}}}"#,
    )]);
    let (status, stdout, _) = invoke(
        &["get-volume", "--volume-id", "volume-1", "--output", "json"],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    let stdout = String::from_utf8(stdout).unwrap();
    assert!(!stdout.contains("1.0") && !stdout.contains("2.0"));
    assert!(stdout.contains("\"Throughput\": 1,"));
}

#[test]
fn volume_writes_refuse_read_only_before_transport() {
    for args in [
        &[
            "attach-volume",
            "--volume-id",
            "volume-1",
            "--server-id",
            "server-1",
        ][..],
        &[
            "create-volume",
            "--name",
            "toy-volume",
            "--zone-id",
            "zone-1",
            "--volume-type-id",
            "type-1",
            "--size",
            "10",
        ][..],
        &["delete-volume", "--volume-id", "volume-1", "--yes"][..],
        &[
            "detach-volume",
            "--volume-id",
            "volume-1",
            "--server-id",
            "server-1",
            "--yes",
        ][..],
        &[
            "resize-volume",
            "--volume-id",
            "volume-1",
            "--size",
            "10",
            "--yes",
        ][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
        assert_eq!(server.requests(), 0);
    }
}

#[test]
fn volume_write_output_matches_go() {
    const AVAILABLE: &str = r#"{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"AVAILABLE","volumeTypeId":"type-1","serverId":null,"serverIdList":[]}}"#;
    const IN_USE: &str = r#"{"data":{"uuid":"volume-1","name":"toy-volume","size":1,"status":"IN-USE","volumeTypeId":"type-1","serverId":"server-1","serverIdList":["server-1"]}}"#;
    const EMPTY: &str = r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#;
    const ACCEPTED: (&str, &str) = ("202 Accepted", "{}");
    const VOLUME: (&str, &str) = ("200 OK", AVAILABLE);
    const PRICE: (&str, &str) = ("200 OK", QUOTE);

    macro_rules! write_goldens {
        ($command:literal, $args:expr, $responses:expr) => {
            for format in ["json", "table", "text"] {
                let expected = match format {
                    "json" => include_str!(concat!("fixtures/volume/", $command, ".json.golden")),
                    "table" => include_str!(concat!("fixtures/volume/", $command, ".table.golden")),
                    _ => include_str!(concat!("fixtures/volume/", $command, ".text.golden")),
                };
                let responses: &[(&str, &str)] = $responses;
                let server = Server::start(
                    responses
                        .iter()
                        .map(|(status, body)| response(status, body))
                        .collect(),
                );
                let args: &[&str] = $args;
                let args: Vec<_> = [$command, "--no-wait", "--output", format]
                    .into_iter()
                    .chain(args.iter().copied())
                    .collect();
                let (status, stdout, stderr) = invoke_mode(&args, &server, false);
                assert_eq!(status, ExitCode::SUCCESS, "{}/{format}", $command);
                assert!(stderr.is_empty());
                assert_eq!(stdout, expected.as_bytes(), "{}/{format}", $command);
                assert_eq!(server.requests(), responses.len());
            }
        };
    }
    write_goldens!(
        "attach-volume",
        &["--volume-id", "volume-1", "--server-id", "server-1"],
        &[VOLUME, ACCEPTED]
    );
    write_goldens!(
        "create-volume",
        &[
            "--name",
            "toy-volume",
            "--zone-id",
            "zone-1",
            "--volume-type-id",
            "type-1",
            "--size",
            "1",
            "--max-price",
            "3200"
        ],
        &[
            ("200 OK", EMPTY),
            PRICE,
            ("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#)
        ]
    );
    write_goldens!(
        "delete-volume",
        &["--volume-id", "volume-1", "--yes"],
        &[VOLUME, ACCEPTED]
    );
    write_goldens!(
        "detach-volume",
        &[
            "--volume-id",
            "volume-1",
            "--server-id",
            "server-1",
            "--yes"
        ],
        &[
            ("200 OK", IN_USE),
            (
                "200 OK",
                r#"{"data":{"status":"STOPPED","bootVolumeId":"boot-1"}}"#
            ),
            ACCEPTED
        ]
    );
    write_goldens!(
        "resize-volume",
        &[
            "--volume-id",
            "volume-1",
            "--size",
            "2",
            "--max-price",
            "3200",
            "--yes"
        ],
        &[VOLUME, PRICE, ACCEPTED]
    );
}

#[test]
fn destructive_volume_commands_need_yes_and_retain_safety_codes() {
    for args in [
        &["delete-volume", "--volume-id", "volume-1"][..],
        &[
            "detach-volume",
            "--volume-id",
            "volume-1",
            "--server-id",
            "server-1",
        ][..],
        &["resize-volume", "--volume-id", "volume-1", "--size", "2"][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, stderr) = invoke_mode(args, &server, false);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("--yes"));
        assert_eq!(server.requests(), 0);
    }
    let attached =
        r#"{"data":{"uuid":"volume-1","size":1,"status":"IN-USE","serverId":"server-1"}}"#;
    for (command, flags, replies, code) in [
        (
            "delete-volume",
            &[][..],
            vec![response("200 OK", attached)],
            "VolumeInUse",
        ),
        (
            "detach-volume",
            &["--server-id", "server-1"][..],
            vec![
                response("200 OK", attached),
                response(
                    "200 OK",
                    r#"{"data":{"status":"STOPPED","bootVolumeId":"volume-1"}}"#,
                ),
            ],
            "BootVolume",
        ),
    ] {
        let count = replies.len();
        let server = Server::start(replies);
        let args: Vec<_> = [command, "--yes", "--volume-id", "volume-1"]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();
        let (status, stdout, stderr) = invoke_mode(&args, &server, false);
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
        assert!(error["error"].get("operation").is_none());
        assert!(error["error"].get("status").is_none());
        assert_eq!(server.requests(), count);
    }
}

#[test]
fn missing_confirmation_status_keeps_unexpected_status_cli_identity() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"data":{"uuid":"volume-1","status":"AVAILABLE"}}"#,
        ),
        response("202 Accepted", ""),
        response("200 OK", r#"{"data":{"uuid":"volume-1"}}"#),
    ]);
    let (status, _, stderr) = invoke_mode(
        &[
            "attach-volume",
            "--volume-id",
            "volume-1",
            "--server-id",
            "server-1",
        ],
        &server,
        false,
    );
    assert_eq!(status, ExitCode::from(1));
    let output: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(output["error"]["code"], "UnexpectedStatus");
    assert!(output["error"].get("status").is_none());
    assert!(output["error"].get("operation").is_none());
    assert_eq!(server.requests(), 3);
}

#[test]
fn paid_write_peer_codes_do_not_select_cli_error_overrides() {
    let local_names = [
        "HistoryIncomplete",
        "QuotaChanged",
        "QuotaStatusIncomplete",
        "SecretFileFailed",
        "RequestFailed",
        "ObjectCopyUnconfirmed",
        "ObjectDeleteUnconfirmed",
        "ObjectWriteUnconfirmed",
        "DiscoveryIncomplete",
        "EstimateLimit",
        "EstimateOverflow",
        "BucketEncryptionIncomplete",
        "PurgeCooldown",
        "StatusUnconfirmed",
        "UnexpectedStatus",
        "ResourceBusy",
        "OTPRejected",
        "ZoneBusy",
        "NotSettled",
        "WriteFailed",
        "OpenLogin",
        "VolumeInUse",
        "BootVolume",
        "ServerRunning",
        "SystemTag",
        "RepositoryNotEmpty",
        "PaymentRequired",
        "ProjectNotEmpty",
        "BucketNotEmpty",
        "UserNotFound",
        "SystemSecurityGroup",
        "SecurityGroupInUse",
        "ServerGroupInUse",
        "ResourceInUse",
        "DefaultResource",
        "PriceAboveMax",
        "Unpriced",
        "SelfChange",
        "PrivilegedChange",
        "ManagedPolicy",
        "NotFound",
        "InvalidUsage",
        "ReadOnly",
        "QueryFailed",
        "NoCredentials",
        "TokenCacheFailed",
        "ServiceAccountLoginFailed",
        "LoginFailed",
        "InvalidConfig",
        "PageFormat",
    ];
    let mut mismatches = Vec::new();
    for code in local_names {
        let rejected = serde_json::json!({"code":code,"message":"provider refused"}).to_string();
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            ),
            response("200 OK", QUOTE),
            response("500 Internal Server Error", &rejected),
        ]);
        let (exit, stdout, stderr) = invoke_mode(
            &[
                "create-volume",
                "--name",
                "toy-volume",
                "--zone-id",
                "zone-1",
                "--volume-type-id",
                "type-1",
                "--size",
                "1",
                "--max-price",
                "3200",
                "--no-wait",
            ],
            &server,
            false,
        );
        let actual: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        let expected = serde_json::json!({"error": {
            "code": code,
            "status": 500,
            "operation": "volume.CreateVolume",
            "message": "provider refused\nCheck for an existing volume with the exact name before retrying.",
        }});
        if exit != ExitCode::FAILURE || !stdout.is_empty() || actual != expected {
            mismatches.push(code);
        }
        assert_eq!(server.requests(), 3);
    }
    assert!(
        mismatches.is_empty(),
        "incorrect CLI metadata or message for peer codes: {mismatches:?}"
    );
}

#[path = "volume/recovery.rs"]
mod recovery;

#[path = "volume/settlement.rs"]
mod settlement;
