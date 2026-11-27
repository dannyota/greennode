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
fn truncated_not_found_is_request_failed() {
    let server = Server::start(vec![Response::RawClose(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 50\r\n\r\nshort".to_vec(),
    )]);
    let options = with_product_loopback(
        Product::Vserver,
        ConfigOptions::default().with_static_token(Secret::new("toy-token")),
        &server,
    );
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: [
            "greennode",
            "--project-id",
            "project-1",
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
    assert_eq!(status, ExitCode::from(1));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "RequestFailed");
    assert_eq!(
        error["error"]["message"],
        "tagging.ListResourceTags: request failed: network request failed"
    );
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

#[test]
fn writes_match_baseline_output_formats() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for (operation, format, expected) in [
        (
            "tag-resource",
            "json",
            &include_bytes!("fixtures/tagging/tag-resource.json.golden")[..],
        ),
        (
            "tag-resource",
            "table",
            &include_bytes!("fixtures/tagging/tag-resource.table.golden")[..],
        ),
        (
            "tag-resource",
            "text",
            &include_bytes!("fixtures/tagging/tag-resource.text.golden")[..],
        ),
        (
            "untag-resource",
            "json",
            &include_bytes!("fixtures/tagging/untag-resource.json.golden")[..],
        ),
        (
            "untag-resource",
            "table",
            &include_bytes!("fixtures/tagging/untag-resource.table.golden")[..],
        ),
        (
            "untag-resource",
            "text",
            &include_bytes!("fixtures/tagging/untag-resource.text.golden")[..],
        ),
    ] {
        let before = r#"[{"key":"env","value":"old","systemTag":false}]"#;
        let after = if operation == "tag-resource" {
            r#"[{"key":"env","value":"new","systemTag":false}]"#
        } else {
            "[]"
        };
        let response = |body: &str| Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.as_bytes().to_vec(),
        };
        let server = Server::start(vec![response(before), response(""), response(after)]);
        let options = with_product_loopback(
            Product::Vserver,
            ConfigOptions::default().with_static_token(Secret::new("toy-token")),
            &server,
        );
        let (context, _) = RequestContext::new();
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let mut args = vec![
            "greennode",
            "--project-id",
            "project-1",
            "--output",
            format,
            "tagging",
            operation,
            "--resource-id",
            "res-1",
            "--resource-type",
            "SERVER",
            "--key",
            "env",
        ];
        if operation == "tag-resource" {
            args.extend(["--value", "new"]);
        }
        let status = runtime.block_on(run_invocation(Invocation {
            args: args.into_iter().map(OsString::from).collect(),
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
        assert_eq!(stdout, expected, "{operation} {format}");
        assert!(stderr.is_empty());
        assert_eq!(server.requests(), 3);
    }
}

#[test]
fn tagging_errors_have_baseline_messages() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for mode in ["system", "confirm-failed", "mismatch"] {
        let response = |body: &str| Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.as_bytes().to_vec(),
        };
        let responses = if mode == "system" {
            vec![]
        } else {
            vec![
                response("[]"),
                response(""),
                response(if mode == "mismatch" { "[]" } else { "[" }),
            ]
        };
        let server = Server::start(responses);
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
                "tagging",
                "tag-resource",
                "--resource-id",
                "res-1",
                "--resource-type",
                "SERVER",
                "--key",
                if mode == "system" { "vng.zone" } else { "env" },
                "--value",
                "new",
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
        assert_eq!(status, ExitCode::from(1));
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        if mode == "system" {
            assert_eq!(error["error"]["code"], "SystemTag");
            assert_eq!(
                error["error"]["message"],
                "tagging: key is a system tag: tagging.TagResource: key \"vng.zone\""
            );
        } else {
            assert_eq!(error["error"]["code"], "NotSettled");
            assert_eq!(
                error["error"]["message"],
                "tagging: write outcome is unconfirmed; read tags before retrying"
            );
        }
    }
}

#[test]
fn system_tag_errors_match_baseline_goldens() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let system = r#"[{"key":"zone","value":"a","systemTag":true}]"#;
    let tag = |key: &'static str| {
        [
            "tag-resource",
            "--resource-id",
            "res-1",
            "--resource-type",
            "SERVER",
            "--key",
            key,
            "--value",
            "new",
        ]
    };
    let untag = |key: &'static str| {
        [
            "untag-resource",
            "--resource-id",
            "res-1",
            "--resource-type",
            "SERVER",
            "--key",
            key,
        ]
    };
    let cases: [(&str, Vec<&str>, Option<&str>); 4] = [
        ("system-tag-prefix", tag("vng.zone").to_vec(), None),
        (
            "system-tag-untag-prefix",
            untag("vng.a\"b\u{85}\u{e9}").to_vec(),
            None,
        ),
        ("system-tag-resource", tag("zone").to_vec(), Some(system)),
        (
            "system-tag-untag-resource",
            untag("zone").to_vec(),
            Some(system),
        ),
    ];
    for (name, args, body) in cases {
        let responses = body
            .map(|body| {
                vec![Response::Http {
                    status: "200 OK".into(),
                    headers: vec![("Content-Length".into(), body.len().to_string())],
                    body: body.as_bytes().to_vec(),
                }]
            })
            .unwrap_or_default();
        let server = Server::start(responses);
        let options = with_product_loopback(
            Product::Vserver,
            ConfigOptions::default().with_static_token(Secret::new("toy-token")),
            &server,
        );
        let (context, _) = RequestContext::new();
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let status = runtime.block_on(run_invocation(Invocation {
            args: ["greennode", "--project-id", "project-1", "tagging"]
                .into_iter()
                .chain(args)
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
        assert_eq!(status, ExitCode::from(1), "{name}");
        assert!(stdout.is_empty(), "{name}");
        let golden = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/tagging/{name}.stderr.golden")),
        )
        .unwrap();
        assert_eq!(String::from_utf8(stderr).unwrap(), golden, "{name}");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_key_is_a_usage_error_without_a_request() {
    use std::os::unix::ffi::OsStringExt;
    let server = Server::start(vec![]);
    let options = with_product_loopback(
        Product::Vserver,
        ConfigOptions::default().with_static_token(Secret::new("toy-token")),
        &server,
    );
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let mut args: Vec<OsString> = [
        "greennode",
        "--project-id",
        "project-1",
        "tagging",
        "tag-resource",
        "--resource-id",
        "res-1",
        "--resource-type",
        "SERVER",
        "--value",
        "new",
        "--key",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(OsString::from_vec(b"vng.\xff".to_vec()));
    let status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_invocation(Invocation {
            args,
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
    assert_eq!(status, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert_eq!(server.requests(), 0);
}
