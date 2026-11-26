use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Response, Server, with_billing_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const BEARER: &str = "synthetic-dns-cli-bearer";
const ZONES: &str = include_str!("../../greennode-sdk/tests/fixtures/dns/list_hosted_zones.json");
const ZONE: &str = include_str!("../../greennode-sdk/tests/fixtures/dns/get_hosted_zone.json");
const RECORDS: &str = include_str!("../../greennode-sdk/tests/fixtures/dns/list_records.json");
const RECORD: &str = include_str!("../../greennode-sdk/tests/fixtures/dns/get_record.json");

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

async fn invoke(server: &Server, bearer: &str, args: &[&str]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let status = run_invocation(Invocation {
        args: ["greennode", "--region", "hcm-3", "dns"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_billing_loopback(
            ConfigOptions::default()
                .with_static_token(Secret::new(bearer))
                .with_endpoint_overrides(EndpointOverrides {
                    dns: Some(format!("{}/vdns-api/", server.origin)),
                    ..Default::default()
                }),
            server,
        ),
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    })
    .await;
    assert!(!String::from_utf8_lossy(&stdout).contains(bearer));
    assert!(!String::from_utf8_lossy(&stderr).contains(bearer));
    (status, stdout, stderr)
}

#[tokio::test]
async fn dns_reads_match_go_goldens_with_read_only_enabled() {
    for (command, route, body, flags) in [
        (
            "list-hosted-zones",
            "hosted-zone?name=example&page=2&size=10",
            ZONES,
            &[
                "--cli-input-json",
                r#"{"Name":"example","Page":1,"Size":10}"#,
                "--page",
                "2",
            ][..],
        ),
        (
            "get-hosted-zone",
            "hosted-zone/zone-1",
            ZONE,
            &["--hosted-zone-id", "zone-1"],
        ),
        (
            "list-records",
            "hosted-zone/zone-1/record?name=www",
            RECORDS,
            &["--hosted-zone-id", "zone-1", "--name", "www"],
        ),
        (
            "get-record",
            "hosted-zone/zone-1/record/record-1",
            RECORD,
            &["--hosted-zone-id", "zone-1", "--record-id", "record-1"],
        ),
    ] {
        let server = Server::start((0..3).map(|_| response("200 OK", body)).collect());
        for format in ["json", "table", "text"] {
            let mut args = vec![
                command,
                "--read-only",
                "--project-id",
                "unused-project",
                "--output",
                format,
            ];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&server, BEARER, &args).await;
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            assert!(stderr.is_empty());
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/dns/{command}.{format}.golden"));
            assert_eq!(stdout, std::fs::read(path).unwrap(), "{command}/{format}");
        }
        assert_eq!(server.requests(), 3);
        assert!(server.heads().iter().all(|head| {
            head.starts_with(format!("GET /vdns-api/v1/dns/{route} ").as_bytes())
                && !head
                    .windows(b"unused-project".len())
                    .any(|part| part == b"unused-project")
        }));
    }
}

#[tokio::test]
async fn get_record_query_uses_the_go_id_field() {
    let server = Server::start(vec![response("200 OK", RECORD)]);
    let (status, stdout, stderr) = invoke(
        &server,
        BEARER,
        &[
            "get-record",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "record-1",
            "--query",
            "Record.ID",
        ],
    )
    .await;
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert_eq!(stdout, b"\"record-1\"\n");
}

#[tokio::test]
async fn query_and_renderers_keep_resource_values_and_redact_reflected_fields() {
    let bearer = "9007199254740992";
    let body = format!(
        r#"{{"listData":[{{"recordId":"record-1","subDomain":"resource-canary","type":"A","value":[{{"value":"reflected {bearer}","weight":9007199254740993}}],"ttl":300}}],"page":1,"pageSize":20,"totalPage":1,"totalItem":1}}"#
    );
    let server = Server::start((0..4).map(|_| response("200 OK", &body)).collect());
    for (format, query) in [
        ("json", None),
        ("json", Some("Items[].{Id:ID,Name:SubDomain,Values:Value}")),
        ("text", None),
        ("table", None),
    ] {
        let mut args = vec![
            "list-records",
            "--hosted-zone-id",
            "zone-1",
            "--output",
            format,
        ];
        if let Some(query) = query {
            args.extend(["--query", query]);
        }
        let (status, stdout, stderr) = invoke(&server, bearer, &args).await;
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("record-1") && stdout.contains("resource-canary"));
        assert_eq!(stdout.matches("redacted").count(), 2);
    }
}

#[tokio::test]
async fn required_and_invalid_path_values_fail_before_request() {
    let server = Server::start(vec![]);
    for args in [
        &["get-record", "--hosted-zone-id", "zone-1"][..],
        &[
            "get-record",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "..",
        ],
        &["list-records", "--hosted-zone-id", "a/b"],
    ] {
        let (status, stdout, stderr) = invoke(&server, BEARER, args).await;
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(status, ExitCode::from(2));
        assert_eq!(error["error"]["code"], "InvalidUsage");
        assert!(stdout.is_empty());
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn incomplete_inventory_exits_one_with_no_stdout() {
    let body = RECORDS.replace(r#""totalPage": 1"#, r#""totalPage": 2"#);
    let server = Server::start(vec![response("200 OK", &body)]);
    let (status, stdout, stderr) = invoke(
        &server,
        BEARER,
        &[
            "list-records",
            "--hosted-zone-id",
            "zone-1",
            "--query",
            "Items",
        ],
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert_eq!(stderr, b"{\"error\":{\"code\":\"RequestFailed\",\"message\":\"dns.ListRecords: incomplete record list; continuation parameters are unverified\"}}\n");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn http_and_decode_failures_keep_go_envelope_fields() {
    let reflected = format!(r#"{{"code":"{BEARER}","message":"reflected {BEARER}"}}"#);
    for (http, body, exit, code, message, status) in [
        (
            "401 Unauthorized",
            "",
            3,
            "RequestFailed",
            "Unauthorized",
            Some(401),
        ),
        ("404 Not Found", "", 4, "NotFound", "Not Found", Some(404)),
        (
            "400 Bad Request",
            r#"{"code":"BadInput","message":"invalid record"}"#,
            1,
            "BadInput",
            "invalid record",
            Some(400),
        ),
        (
            "400 Bad Request",
            reflected.as_str(),
            1,
            "RequestFailed",
            "Bad Request",
            Some(400),
        ),
        (
            "200 OK",
            "{",
            1,
            "RequestFailed",
            "response failed to decode",
            None,
        ),
        (
            "200 OK",
            r#"{"data":{"ttl":"invalid"}}"#,
            1,
            "RequestFailed",
            "response failed to decode",
            None,
        ),
    ] {
        let server = Server::start(vec![response(http, body)]);
        let (actual, stdout, stderr) = invoke(
            &server,
            BEARER,
            &[
                "get-record",
                "--hosted-zone-id",
                "zone-1",
                "--record-id",
                "record-1",
            ],
        )
        .await;
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(actual, ExitCode::from(exit), "{http}");
        assert!(stdout.is_empty());
        assert_eq!(error["error"]["code"], code);
        assert_eq!(error["error"]["message"], message);
        assert_eq!(error["error"]["operation"], "dns.GetRecord");
        assert_eq!(error["error"]["status"].as_u64(), status);
    }
}

#[tokio::test]
async fn dns_writes_match_go_goldens() {
    const CREATE_ZONE: &str =
        include_str!("../../greennode-sdk/tests/fixtures/dns/create_hosted_zone.json");
    const CREATE_RECORD: &str =
        include_str!("../../greennode-sdk/tests/fixtures/dns/create_record.json");
    let snapshot = CREATE_ZONE.replace("CREATING", "ACTIVE");
    let replaced = snapshot.replace("<description>", "new description");
    for (command, flags, sequence) in [
        (
            "create-hosted-zone",
            &[
                "--domain-name",
                "example.test",
                "--vpc-ids",
                "vpc-1",
                "--vpc-ids",
                "vpc-2",
            ][..],
            vec![("200 OK", CREATE_ZONE)],
        ),
        (
            "update-hosted-zone",
            &[
                "--hosted-zone-id",
                "zone-1",
                "--description",
                "new description",
            ],
            vec![
                ("200 OK", &snapshot),
                ("204 No Content", ""),
                ("200 OK", &replaced),
            ],
        ),
        (
            "delete-hosted-zone",
            &["--hosted-zone-id", "zone-1", "--yes"],
            vec![("200 OK", ZONE), ("204 No Content", "")],
        ),
        (
            "create-record",
            &[
                "--hosted-zone-id",
                "zone-1",
                "--type",
                "TXT",
                "--cli-input-json",
                r#"{"Values":[{"Value":"sample text","Weight":0}]}"#,
            ],
            vec![("200 OK", ZONE), ("200 OK", CREATE_RECORD)],
        ),
        (
            "update-record",
            &[
                "--hosted-zone-id",
                "zone-1",
                "--record-id",
                "record-1",
                "--ttl",
                "0",
                "--sticky-session=false",
                "--cli-input-json",
                r#"{"TTL":300,"Values":[]}"#,
            ],
            vec![("200 OK", ZONE), ("204 No Content", ""), ("200 OK", RECORD)],
        ),
        (
            "delete-record",
            &[
                "--hosted-zone-id",
                "zone-1",
                "--record-id",
                "record-1",
                "--yes",
            ],
            vec![("200 OK", ZONE), ("204 No Content", "")],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let server = Server::start(
                sequence
                    .iter()
                    .map(|(status, body)| response(status, body))
                    .collect(),
            );
            let mut args = vec![command, "--no-wait", "--output", format];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&server, BEARER, &args).await;
            assert!(
                stderr.is_empty(),
                "{command}/{format}: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert_eq!(status, ExitCode::SUCCESS, "{command}/{format}");
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/dns/{command}.{format}.golden"));
            assert_eq!(stdout, std::fs::read(path).unwrap(), "{command}/{format}");
            assert_eq!(server.requests(), sequence.len());
            if command == "update-record" {
                let body: serde_json::Value = serde_json::from_slice(&server.bodies()[1]).unwrap();
                assert_eq!(
                    body,
                    serde_json::json!({"ttl":0,"enableStickySession":false,"value":[]})
                );
            }
            if command == "create-hosted-zone" {
                let body: serde_json::Value = serde_json::from_slice(&server.bodies()[0]).unwrap();
                assert_eq!(body["assocVpcIds"], serde_json::json!(["vpc-1", "vpc-2"]));
            }
        }
    }
}

#[tokio::test]
async fn dns_write_guards_stop_before_preflight() {
    let server = Server::start(vec![]);
    for args in [
        &[
            "create-hosted-zone",
            "--read-only",
            "--domain-name",
            "example.test",
            "--vpc-ids",
            "vpc-1",
        ][..],
        &[
            "update-hosted-zone",
            "--read-only",
            "--hosted-zone-id",
            "zone-1",
            "--description",
            "new",
        ],
        &[
            "create-record",
            "--read-only",
            "--hosted-zone-id",
            "zone-1",
            "--type",
            "A",
            "--cli-input-json",
            r#"{"Values":[{"Value":"192.0.2.1"}]}"#,
        ],
        &[
            "update-record",
            "--read-only",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "record-1",
            "--ttl",
            "30",
        ],
        &["delete-hosted-zone", "--hosted-zone-id", "zone-1"],
        &[
            "delete-record",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "record-1",
        ],
        &[
            "delete-hosted-zone",
            "--read-only",
            "--hosted-zone-id",
            "zone-1",
            "--yes",
        ],
        &[
            "delete-record",
            "--read-only",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "record-1",
            "--yes",
        ],
    ] {
        let (status, stdout, stderr) = invoke(&server, BEARER, args).await;
        assert_ne!(status, ExitCode::SUCCESS);
        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains(if args.contains(&"--read-only") {
            "ReadOnly"
        } else {
            "--yes"
        }));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn partial_dns_write_output_ignores_query_and_omits_nested_http_status() {
    let server = Server::start(vec![
        response("200 OK", ZONE),
        response("204 No Content", ""),
        response("404 Not Found", ""),
    ]);
    let (status, stdout, stderr) = invoke(
        &server,
        BEARER,
        &[
            "update-record",
            "--hosted-zone-id",
            "zone-1",
            "--record-id",
            "record-1",
            "--ttl",
            "40",
            "--no-wait",
            "--query",
            "Record.TTL",
        ],
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    let partial: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(partial["Record"]["ID"], "record-1");
    assert_eq!(partial["Record"]["TTL"], 40);
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotSettled");
    assert_eq!(
        error["error"]["message"],
        "vngcloud: write outcome not confirmed: dns.UpdateRecord: record record-1: dns.UpdateRecord: Not Found (status 404)"
    );
    assert!(error["error"].get("status").is_none());
    assert!(error["error"].get("operation").is_none());
}
