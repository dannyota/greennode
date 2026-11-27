use super::{Source, response};
use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Server, with_write_loopback};
use greennode_sdk::{ConfigOptions, RequestContext, Secret, SystemClock};
use std::{ffi::OsString, process::ExitCode};

const TOKEN: &str = "toy-monitor-write-token";
fn invoke(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let context = RequestContext::new().0;
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let options = ConfigOptions::default().with_static_token(Secret::new(TOKEN));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "monitor"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_write_loopback(Product::Monitor, options, server),
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    assert!(!String::from_utf8_lossy(&stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&stderr).contains(TOKEN));
    (status, stdout, stderr)
}
macro_rules! fixture {
    ($file:literal) => {
        include_str!(concat!(
            "../../../greennode-sdk/tests/fixtures/monitor/",
            $file,
            ".json"
        ))
    };
}
macro_rules! golden {
    ($op:literal) => {
        [
            include_str!(concat!("../fixtures/monitor/", $op, ".json.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".table.golden")),
            include_str!(concat!("../fixtures/monitor/", $op, ".text.golden")),
        ]
    };
}
#[test]
fn channel_writes_match_go_and_mask_file_credentials_before_queries_and_rendering() {
    let path = std::env::temp_dir().join(format!(
        "greennode-monitor-channel-{}.json",
        getrandom::u64().unwrap()
    ));
    std::fs::write(&path,r#"{"Address":"https://example.test/secret-path","Headers":[{"Key":"X-Hook","Value":"toy-hook"}]}"#).unwrap();
    let address = format!("file://{}", path.display());
    for (operation, flags, body, goldens) in [
        (
            "create-channel",
            &[
                "--name",
                "Alerts",
                "--type",
                "Webhook",
                "--cli-input-json",
                address.as_str(),
            ][..],
            &[fixture!("created_channel")][..],
            golden!("create-channel"),
        ),
        (
            "update-channel",
            &["--channel-id", "channel-1", "--name", "Changed"][..],
            &[fixture!("remaining_channels"), ""][..],
            golden!("update-channel"),
        ),
        (
            "delete-channel",
            &["--channel-id", "channel-1", "--yes"][..],
            &[""][..],
            golden!("delete-channel"),
        ),
        (
            "send-channel-otp",
            &["--type", "Email", "--address", "example@example.test"][..],
            &[fixture!("sent_channel_otp")][..],
            golden!("send-channel-otp"),
        ),
    ] {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(body.iter().map(|b| response(b)).collect());
            let mut args = vec![operation, "--output", format];
            args.extend(flags);
            let (status, stdout, stderr) = invoke(&args, &server);
            assert_eq!(status, ExitCode::SUCCESS, "{operation}/{format}");
            assert!(stderr.is_empty());
            assert!(stdout == golden.as_bytes(), "{operation}/{format}");
            assert_eq!(server.requests(), body.len());
        }
    }
    std::fs::remove_file(path).unwrap();
}
#[test]
fn literal_channel_secrets_and_destructive_or_read_only_guard_failures_send_nothing() {
    for args in [
        &[
            "create-channel",
            "--name",
            "x",
            "--type",
            "Webhook",
            "--address",
            "https://example.test/label",
        ][..],
        &[
            "update-channel",
            "--channel-id",
            "channel-1",
            "--cli-input-json",
            r#"{"Address":"redacted-value"}"#,
        ][..],
        &[
            "send-channel-otp",
            "--type",
            "Email",
            "--address",
            "example@example.test",
            "--read-only",
        ][..],
        &["delete-channel", "--channel-id", "channel-1"][..],
    ] {
        let server = Server::start(vec![]);
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(!String::from_utf8_lossy(&stderr).contains("redacted-value"));
        assert_eq!(server.requests(), 0);
    }
}
