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
struct Case {
    command: &'static str,
    flags: &'static [&'static str],
    response: &'static str,
    goldens: [&'static [u8]; 3],
}
fn invoke(args: &[&str], server: &Server) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let config_options = with_product_loopback(
        Product::Database,
        ConfigOptions::default().with_static_token(Secret::new("toy-cli-bearer")),
        server,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let args = std::iter::once("greennode")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect();
    let status = runtime.block_on(run_invocation(Invocation {
        args,
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
    (status, stdout, stderr)
}

#[test]
fn database_json_pagination_and_local_guards() {
    let body = include_bytes!("fixtures/database/list-database-instances.response.json");
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }]);
    let (status, _, stderr) = invoke(
        &[
            "database",
            "list-database-instances",
            "--cli-input-json",
            r#"{"PageNumber":2,"PageSize":5}"#,
        ],
        &server,
    );
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(
        server.heads()[0]
            .starts_with(b"GET /vdb-relational/v1/database-instances?pageNumber=2&pageSize=5 ")
    );
    for args in [
        &["database", "list-database-flavors"][..],
        &[
            "database",
            "list-database-zones",
            "--cli-input-json",
            "file://",
        ][..],
        &[
            "database",
            "list-database-instances",
            "--cli-input-json",
            r#"{"PageNumber":2147483648}"#,
        ][..],
    ] {
        let (status, stdout, _) = invoke(args, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
    }
    assert_eq!(server.requests(), 1);
}

#[test]
fn database_matches_baseline_goldens() {
    let cases = [
        Case {
            command: "get-database-instance",
            flags: &["--id", "db_one"],
            response: include_str!("fixtures/database/get-database-instance.response.json"),
            goldens: [
                include_bytes!("fixtures/database/get-database-instance.json.golden"),
                include_bytes!("fixtures/database/get-database-instance.table.golden"),
                include_bytes!("fixtures/database/get-database-instance.text.golden"),
            ],
        },
        Case {
            command: "list-database-engine-versions",
            flags: &[],
            response: include_str!("fixtures/database/list-database-engine-versions.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-engine-versions.json.golden"),
                include_bytes!("fixtures/database/list-database-engine-versions.table.golden"),
                include_bytes!("fixtures/database/list-database-engine-versions.text.golden"),
            ],
        },
        Case {
            command: "list-database-zones",
            flags: &[],
            response: include_str!("fixtures/database/list-database-zones.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-zones.json.golden"),
                include_bytes!("fixtures/database/list-database-zones.table.golden"),
                include_bytes!("fixtures/database/list-database-zones.text.golden"),
            ],
        },
        Case {
            command: "list-database-flavors",
            flags: &[
                "--engine",
                "mysql",
                "--engine-version",
                "8.0",
                "--zone-id",
                "zone-1",
            ],
            response: include_str!("fixtures/database/list-database-flavors.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-flavors.json.golden"),
                include_bytes!("fixtures/database/list-database-flavors.table.golden"),
                include_bytes!("fixtures/database/list-database-flavors.text.golden"),
            ],
        },
        Case {
            command: "list-database-storage-types",
            flags: &["--zone-id", "zone-1"],
            response: include_str!("fixtures/database/list-database-storage-types.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-storage-types.json.golden"),
                include_bytes!("fixtures/database/list-database-storage-types.table.golden"),
                include_bytes!("fixtures/database/list-database-storage-types.text.golden"),
            ],
        },
        Case {
            command: "list-database-networks",
            flags: &["--zone-id", "zone-1"],
            response: include_str!("fixtures/database/list-database-networks.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-networks.json.golden"),
                include_bytes!("fixtures/database/list-database-networks.table.golden"),
                include_bytes!("fixtures/database/list-database-networks.text.golden"),
            ],
        },
        Case {
            command: "list-database-instances",
            flags: &[],
            response: include_str!("fixtures/database/list-database-instances.response.json"),
            goldens: [
                include_bytes!("fixtures/database/list-database-instances.json.golden"),
                include_bytes!("fixtures/database/list-database-instances.table.golden"),
                include_bytes!("fixtures/database/list-database-instances.text.golden"),
            ],
        },
    ];
    for case in cases {
        let body = case.response.as_bytes();
        let server = Server::start(
            (0..3)
                .map(|_| Response::Http {
                    status: "200 OK".into(),
                    headers: vec![("Content-Length".into(), body.len().to_string())],
                    body: body.to_vec(),
                })
                .collect(),
        );
        for (index, format) in ["json", "table", "text"].into_iter().enumerate() {
            let (context, _) = RequestContext::new();
            let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
            let options = with_product_loopback(
                Product::Database,
                ConfigOptions::default().with_static_token(Secret::new("toy-cli-bearer")),
                &server,
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let args = ["greennode", "database", case.command, "--output", format]
                .into_iter()
                .chain(case.flags.iter().copied())
                .map(OsString::from)
                .collect();
            let status = runtime.block_on(run_invocation(Invocation {
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
            assert_eq!(
                status,
                ExitCode::SUCCESS,
                "{} {format}: {}",
                case.command,
                String::from_utf8_lossy(&stderr)
            );
            assert_eq!(stdout, case.goldens[index], "{} {format}", case.command);
            assert!(stderr.is_empty());
        }
        assert_eq!(server.requests(), 3);
        if case.command == "get-database-instance" {
            assert!(server.heads().iter().all(|head| {
                head.starts_with(b"GET /vdb-relational/v1/database-instances/id/db_one ")
            }));
        }
    }
}
