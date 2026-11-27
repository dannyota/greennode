#[path = "cdn/writes.rs"]
mod writes;

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{
    ConfigOptions, ConfigSource, EndpointOverrides, RequestContext, Secret, SystemClock,
};

const KEY: &str = "<secret>";
const DOMAIN: &[&str] = &[
    "--cli-input-json",
    r#"{"CDNDomains":["cdn.example.test"]}"#,
    "--period",
    "24h",
];
const REPORT: &[&str] = &[
    "--cli-input-json",
    r#"{"CDNDomains":["cdn.example.test"]}"#,
    "--from",
    "2026-10-08",
    "--to",
    "2026-10-09",
];

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "hcm-3".into())
    }
}

fn invoke(
    args: &[&str],
    server: &Server,
    docs: bool,
    key: Option<&str>,
    management: bool,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let mut config_options = with_product_loopback(
        if docs { Product::CdnDocs } else { Product::Cdn },
        ConfigOptions::default().with_endpoint_overrides(EndpointOverrides {
            cdn_docs: Some(format!("{}/faq/vcdn", server.origin)),
            cdn: Some(format!("{}/vcdn-api/", server.origin)),
            ..Default::default()
        }),
        server,
    );
    if let Some(key) = key {
        config_options = config_options.with_cdn_api_key(Secret::new(key));
    }
    if management {
        config_options = config_options.with_static_token(Secret::new("toy-management-token"));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: std::iter::once("greennode")
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
        assert!(
            !text.contains(KEY)
                && !text.contains("toy-management-token")
                && !text.contains("PRIVATE KEY")
        );
    }
    (status, stdout, stderr)
}

fn response(status: &str, body: &str, content_type: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![
            ("Content-Length".into(), body.len().to_string()),
            ("Content-Type".into(), content_type.into()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

macro_rules! fixture {
    ($operation:literal, $response:literal, $flags:expr) => {
        (
            $operation,
            include_str!(concat!(
                "../../greennode-sdk/tests/fixtures/cdn/",
                $response
            )),
            $flags,
            [
                include_str!(concat!("fixtures/cdn/", $operation, ".json.golden")),
                include_str!(concat!("fixtures/cdn/", $operation, ".table.golden")),
                include_str!(concat!("fixtures/cdn/", $operation, ".text.golden")),
            ],
        )
    };
}

#[test]
fn web_accelerator_refuses_when_redaction_would_publish_the_key() {
    let server = Server::start(vec![response(
        "200 OK",
        r#"{"success":true,"data":{"cdnId":"cdn-1","status":1,"domainName":"redacted"}}"#,
        "application/json",
    )]);
    let (status, stdout, _stderr) = invoke(
        &[
            "--read-only",
            "cdn",
            "get-web-accelerator",
            "--cdn-id",
            "cdn-1",
        ],
        &server,
        false,
        Some("redacted"),
        true,
    );
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert_eq!(server.requests(), 1);
}

#[test]
fn all_cdn_reads_match_the_go_renderer() {
    let cases = [
        fixture!("get-cache-status", "analytics-cache-status.json", DOMAIN),
        fixture!(
            "get-certificate",
            "certificate-detail.json",
            &["--certificate-id", "cert-1"][..]
        ),
        fixture!("get-http-codes", "analytics-http-codes.json", DOMAIN),
        fixture!("get-request-rate", "analytics-request-rate.json", DOMAIN),
        fixture!("get-traffic", "analytics-traffic.json", DOMAIN),
        fixture!(
            "get-traffic-report",
            "analytics-traffic-report.json",
            REPORT
        ),
        fixture!(
            "get-web-accelerator",
            "webaccelerator-detail.json",
            &["--cdn-id", "cdn-1"][..]
        ),
        fixture!("list-api-keys", "apikey-list.json", &[][..]),
        fixture!("list-certificates", "certificate-list.json", &[][..]),
        fixture!("list-ip-ranges", "faq-vcdn.html", &[][..]),
        fixture!("list-web-accelerators", "webaccelerator-list.json", &[][..]),
    ];
    let targets = [
        "POST /vcdn-api/v1/analytic/cache-status ",
        "GET /vcdn-api/v1/certificate/detail/cert-1 ",
        "POST /vcdn-api/v1/analytic/cdn-http-codes ",
        "POST /vcdn-api/v1/analytic/cdn-requestsps ",
        "POST /vcdn-api/v1/analytic/traffic-consuming ",
        "POST /vcdn-api/v1/analytic/traffic-report ",
        "GET /vcdn-api/v1/cdn/detail/cdn-1 ",
        "GET /vcdn-api/v1/apikey/list ",
        "GET /vcdn-api/v1/certificate/list ",
        "GET /faq/vcdn ",
        "GET /vcdn-api/v1/cdn/list ",
    ];
    for ((operation, body, flags, goldens), target) in cases.into_iter().zip(targets) {
        let docs = operation == "list-ip-ranges";
        let media = if docs {
            "text/html"
        } else {
            "application/json"
        };
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(vec![response("200 OK", body, media)]);
            let mut args = vec!["--read-only", "--output", format, "cdn", operation];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) =
                invoke(&args, &server, docs, (!docs).then_some(KEY), true);
            assert_eq!(status, ExitCode::SUCCESS, "{operation} {format}");
            assert!(stderr.is_empty(), "{operation} {format}");
            let normalized = String::from_utf8(stdout)
                .unwrap()
                .replace(&server.origin, "https://docs.greennode.ai");
            assert_eq!(normalized, golden, "{operation} {format}");
            assert_eq!(server.requests(), 1);
            let heads = server.heads();
            assert!(heads[0].starts_with(target.as_bytes()), "{operation}");
            let head = String::from_utf8_lossy(&heads[0]).to_ascii_lowercase();
            assert_eq!(head.contains("authorization:"), !docs);
            if !docs {
                assert!(head.contains("authorization: bearer <secret>"));
            }
            assert!(!head.contains("toy-management-token") && !head.contains("\r\norigin:"));
        }
    }
}

#[test]
fn invalid_credentials_and_public_page_have_distinct_errors() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke(&["cdn", "list-api-keys"], &server, false, None, true);
    assert_eq!(status, ExitCode::from(3));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("configure set vcdn_api_key -")
    );
    assert!(error["error"]["operation"].is_null());
    assert_eq!(server.requests(), 0);

    let (status, _, stderr) = invoke(&["cdn", "list-api-keys"], &server, false, Some(KEY), false);
    assert_eq!(status, ExitCode::from(3));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
    assert_eq!(server.requests(), 0);

    let server = Server::start(vec![response(
        "200 OK",
        "<h1>Changed format</h1>",
        "text/html",
    )]);
    let (status, _, stderr) = invoke(&["cdn", "list-ip-ranges"], &server, true, None, true);
    assert_eq!(status, ExitCode::from(1));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "PageFormat");
    assert!(error["error"]["status"].is_null());
}

#[test]
fn public_command_requires_management_configuration_without_sending_a_bearer() {
    let server = Server::start(vec![]);
    let (status, stdout, stderr) = invoke(&["cdn", "list-ip-ranges"], &server, true, None, false);
    assert_eq!(status, ExitCode::from(3));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
    assert_eq!(server.requests(), 0);
}

#[test]
fn cdn_http_failures_keep_the_baseline_exit_codes() {
    let cases = [
        (
            "list-ip-ranges",
            "401 Unauthorized",
            String::new(),
            true,
            3,
            401,
            "Unauthorized",
            "cdn.ListIPRanges",
        ),
        (
            "list-web-accelerators",
            "400 Bad Request",
            "x".repeat((8 << 20) + 1),
            false,
            2,
            400,
            "BadRequest",
            "cdn.ListWebAccelerators",
        ),
    ];
    for (op, status, body, docs, exit, http, code, sdk_op) in cases {
        let server = Server::start(vec![response(status, &body, "text/plain")]);
        let (status, stdout, stderr) =
            invoke(&["cdn", op], &server, docs, (!docs).then_some(KEY), true);
        assert_eq!(status, ExitCode::from(exit), "{op}");
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], code);
        assert_eq!(envelope["error"]["status"], http);
        assert_eq!(envelope["error"]["operation"], sdk_op);
    }
}
