use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};

const TOKEN: &str = "toy-monitor-token";

struct Source;

impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "hcm-3".into())
    }
}

fn invoke(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let options = ConfigOptions::default().with_static_token(Secret::new(TOKEN));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "--read-only", "monitor"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_product_loopback(Product::Monitor, options, server),
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
        assert!(!String::from_utf8_lossy(stream).contains(TOKEN));
    }
    (status, stdout, stderr)
}

macro_rules! body {
    ($file:literal) => {
        include_str!(concat!(
            "../../greennode-sdk/tests/fixtures/monitor/",
            $file,
            ".json"
        ))
    };
}

macro_rules! case {
    ($operation:literal, $flags:expr, $responses:expr) => {
        (
            $operation,
            $flags,
            $responses,
            [
                include_str!(concat!("fixtures/monitor/", $operation, ".json.golden")),
                include_str!(concat!("fixtures/monitor/", $operation, ".table.golden")),
                include_str!(concat!("fixtures/monitor/", $operation, ".text.golden")),
            ],
        )
    };
}

fn response(body: &str) -> Response {
    Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn monitor_reads_match_go_goldens_in_every_format() {
    for (operation, flags, responses, goldens) in [
        case!(
            "get-dashboard",
            &["--dashboard-id", "dash-1"][..],
            &[body!("get_dashboard_system")][..]
        ),
        case!(
            "list-dashboards",
            &["--system"][..],
            &[body!("list_dashboards_system")][..]
        ),
        case!(
            "list-check-response-times",
            &[
                "--check-id",
                "chk-1",
                "--start",
                "2000-01-01T00:00:00Z",
                "--end",
                "2000-01-02T00:00:00Z"
            ][..],
            &[
                body!("response_time_check"),
                body!("ListCheckResponseTimes")
            ][..]
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(responses.iter().map(|data| response(data)).collect());
            let mut args = vec![operation, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty(), "{operation}/{format}");
            assert_eq!(stdout, golden.as_bytes(), "{operation}/{format}");
            assert_eq!(server.requests(), responses.len());
        }
    }
}

#[test]
fn dashboard_rejects_reflected_name_before_query_output() {
    let reflected = body!("get_dashboard_system").replace("Synthetic", "toy%2Dmonitor%2Dtoken");
    let server = Server::start(vec![response(&reflected)]);
    let (status, output, stderr) = invoke(
        &[
            "get-dashboard",
            "--dashboard-id",
            "dash-1",
            "--query",
            "Dashboard.Name",
        ],
        &server,
    );
    assert_ne!(status, ExitCode::SUCCESS);
    assert!(output.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("InvalidResponse"));
}
