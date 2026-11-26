use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};
use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

struct Source;
impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        match name.to_str()? {
            "GREENNODE_REGION" => Some("hcm-3".into()),
            _ => None,
        }
    }
}

#[test]
fn list_matches_baseline_output_formats() {
    let body = include_str!("../../greennode-sdk/tests/fixtures/tagging/list_resource_tags.json");
    let server = Server::start(
        (0..3)
            .map(|_| Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.as_bytes().to_vec(),
            })
            .collect(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for (format, expected) in [
        (
            "json",
            &include_bytes!("fixtures/tagging/list-resource-tags.json.golden")[..],
        ),
        (
            "table",
            &include_bytes!("fixtures/tagging/list-resource-tags.table.golden")[..],
        ),
        (
            "text",
            &include_bytes!("fixtures/tagging/list-resource-tags.text.golden")[..],
        ),
    ] {
        let options = with_product_loopback(
            Product::Vserver,
            ConfigOptions::default().with_static_token(Secret::new("toy-token")),
            &server,
        );
        let (context, _) = RequestContext::new();
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let status = runtime.block_on(run_invocation(Invocation {
            args: [
                "greennode",
                "--project-id",
                "project-1",
                "--output",
                format,
                "tagging",
                "list-resource-tags",
                "--resource-id",
                "res-1",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            source: &Source,
            config_options: options,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context: &context,
        }));
        assert_eq!(status, ExitCode::SUCCESS);
        assert_eq!(stdout, expected);
        assert!(stderr.is_empty());
    }
    assert_eq!(server.requests(), 3);
}
