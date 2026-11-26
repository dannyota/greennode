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
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_billing_loopback(
        ConfigOptions::default()
            .with_static_token(Secret::new(BEARER))
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
            .chain(["--project-id", "proj-1", "--read-only", "volume"])
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
