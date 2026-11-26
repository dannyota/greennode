use greennode::cli::{Invocation, Stdin, run_invocation};
use greennode_sdk::testing::{
    Product, Response, Server, with_product_loopback, with_write_loopback,
};
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
    invoke_using(args, server, false, &mut std::io::empty())
}

fn invoke_using(
    args: &[&str],
    server: &Server,
    write: bool,
    stdin: &mut dyn Stdin,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let (context, _) = RequestContext::new();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let configure = if write {
        with_write_loopback
    } else {
        with_product_loopback
    };
    let config_options = configure(
        Product::Database,
        ConfigOptions::default()
            .with_project_id("project-1".into())
            .with_static_token(Secret::new("toy-cli-bearer")),
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
        stdin,
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
fn database_errors_match_baseline() {
    let replies: [(&str, &[u8]); 2] = [
        ("403 Forbidden", br#"{"message":"private"}"#),
        ("200 OK", br#"{"code":200,"data":null}"#),
    ];
    let server = Server::start(
        replies
            .into_iter()
            .map(|(status, body)| Response::Http {
                status: status.into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.to_vec(),
            })
            .collect(),
    );
    for (expected_status, expected_code, expected_message) in [
        (403, "Forbidden", "request failed; response withheld"),
        (200, "InvalidResponse", "invalid response; body withheld"),
    ] {
        let (status, stdout, stderr) = invoke(
            &["database", "get-database-instance", "--id", "db_one"],
            &server,
        );
        assert_eq!(status, ExitCode::from(1));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["operation"], "GetDatabaseInstance");
        assert_eq!(error["error"]["status"], expected_status);
        assert_eq!(error["error"]["code"], expected_code);
        assert_eq!(error["error"]["message"], expected_message);
    }
    let (status, _, stderr) = invoke(
        &["database", "get-database-instance", "--id", "../invalid"],
        &server,
    );
    assert_eq!(status, ExitCode::from(2));
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "InvalidUsage");
    assert_eq!(
        error["error"]["message"],
        "vngcloud: invalid input: invalid database selection"
    );
    assert_eq!(server.requests(), 2);
}

#[test]
fn database_null_replicas_render_as_empty_arrays() {
    let list = br#"{"code":200,"data":{"data":[{"id":"db_one","name":"db","status":"ACTIVE","replicas":null}],"pageObject":{}}}"#;
    let detail =
        br#"{"code":200,"data":{"id":"db_one","name":"db","status":"ACTIVE","replicas":null}}"#;
    let server = Server::start(
        [list.as_slice(), detail.as_slice()]
            .into_iter()
            .map(|body| Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.to_vec(),
            })
            .collect(),
    );
    for (args, key) in [
        (&["database", "list-database-instances"][..], "Items"),
        (
            &["database", "get-database-instance", "--id", "db_one"][..],
            "Instance",
        ),
    ] {
        let (status, stdout, stderr) = invoke(args, &server);
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let instance = if key == "Items" {
            &output[key][0]
        } else {
            &output[key]
        };
        assert_eq!(instance["Replicas"], serde_json::json!([]));
    }
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

const CREATE_JSON: &str = r#"{"Name":"demo-db","ZoneID":"zone-1","FlavorID":1,"StorageTypeID":"SSD","StorageSize":20,"VPCID":"network-1","SubnetID":"subnet-1","DatabaseName":"appdb","Username":"admin"}"#;
const PASSWORD: &str = "Synthetic_Database32";
const DETAIL: &[u8] = include_bytes!("fixtures/database/create-database-instance.response.json");

fn create_args<'a>(path: &'a str, format: &'a str) -> [&'a str; 9] {
    [
        "database",
        "create-database-instance",
        "--yes",
        "--cli-input-json",
        CREATE_JSON,
        "--password-file",
        path,
        "--output",
        format,
    ]
}

fn http(status: &str, body: &[u8]) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }
}

fn create_replies(detail: &[u8]) -> Vec<Response> {
    [
        include_bytes!("fixtures/database/list-database-engine-versions.response.json").as_slice(),
        include_bytes!("fixtures/database/list-database-zones.response.json"),
        include_bytes!("fixtures/database/list-database-flavors.response.json"),
        include_bytes!("fixtures/database/list-database-storage-types.response.json"),
        include_bytes!("fixtures/database/list-database-networks.response.json"),
        br#"{"code":200,"data":[{"resourceId":"db-1"}]}"#,
        detail,
    ]
    .map(|body| http("200 OK", body))
    .into()
}

fn delete_replies(confirmation: Response) -> Vec<Response> {
    vec![
        http("200 OK", DETAIL),
        http("200 OK", br#"{"code":200,"data":{"projectId":"project-1","data":[{"id":"db-1","name":"demo-db","status":"ACTIVE"}],"pageObject":{"number":1,"size":1,"totalElements":1,"totalPages":1}}}"#),
        http("200 OK", include_bytes!("fixtures/database/list-database-zones.response.json")),
        http("200 OK", br#"{"code":200,"data":[]}"#),
        http("204 No Content", b""),
        confirmation,
    ]
}

#[test]
fn database_write_guards_run_before_requests_or_password_reads() {
    let server = Server::start(vec![]);
    let create = [
        "database",
        "create-database-instance",
        "--cli-input-json",
        CREATE_JSON,
        "--password-file",
        "missing-password-file",
    ];
    let delete = ["database", "delete-database-instance", "--id", "db-1"];
    for command in [&create[..], &delete[..]] {
        let (status, stdout, stderr) = invoke(command, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("--yes"));

        let mut read_only = vec!["--yes", "--read-only"];
        read_only.extend_from_slice(command);
        let (status, stdout, stderr) = invoke(&read_only, &server);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
    assert_eq!(server.requests(), 0);
}

#[test]
fn database_write_success_matches_all_go_renderers() {
    struct PasswordDirectory(std::path::PathBuf);
    impl Drop for PasswordDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let path =
        std::env::temp_dir().join(format!("greennode-database-writes-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let temp = PasswordDirectory(path);
    let path = temp.0.join("password");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).unwrap();
    assert!(std::io::Write::write_all(&mut file, PASSWORD.as_bytes()).is_ok());
    drop(file);
    let line = format!("{PASSWORD}\r\n");
    for (index, format) in ["json", "table", "text"].into_iter().enumerate() {
        let server = Server::start(create_replies(DETAIL));
        let password_path = if index == 0 {
            path.to_str().unwrap()
        } else {
            "-"
        };
        let (status, stdout, stderr) = invoke_using(
            &create_args(password_path, format),
            &server,
            true,
            &mut line.as_bytes(),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        assert_eq!(
            stdout,
            [
                include_bytes!("fixtures/database/create-database-instance.json.golden").as_slice(),
                include_bytes!("fixtures/database/create-database-instance.table.golden"),
                include_bytes!("fixtures/database/create-database-instance.text.golden"),
            ][index]
        );
        assert_eq!(server.requests(), 7);
        let post: serde_json::Value = serde_json::from_slice(&server.bodies()[5]).unwrap();
        assert!(post["user"]["password"].as_str() == Some(PASSWORD));

        let server = Server::start(delete_replies(http("404 Not Found", b"{}")));
        let (status, stdout, stderr) = invoke_using(
            &[
                "database",
                "delete-database-instance",
                "--id",
                "db-1",
                "--yes",
                "--output",
                format,
            ],
            &server,
            true,
            &mut std::io::empty(),
        );
        assert_eq!(status, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        assert_eq!(
            stdout,
            [
                include_bytes!("fixtures/database/delete-database-instance.json.golden").as_slice(),
                include_bytes!("fixtures/database/delete-database-instance.table.golden"),
                include_bytes!("fixtures/database/delete-database-instance.text.golden"),
            ][index]
        );
        assert_eq!(server.requests(), 6);
    }
}

#[test]
fn database_partial_outputs_match_go_without_querying_unconfirmed_data() {
    let failed = String::from_utf8(DETAIL.to_vec())
        .unwrap()
        .replace("ACTIVE", "FAILED");
    let server = Server::start(create_replies(failed.as_bytes()));
    let mut args = create_args("-", "json").to_vec();
    args.extend(["--query", "Instance.Name"]);
    let (status, stdout, stderr) = invoke_using(&args, &server, true, &mut PASSWORD.as_bytes());
    assert_eq!(status, ExitCode::FAILURE);
    assert_eq!(
        stdout,
        include_bytes!("fixtures/database/create-database-instance.partial.json.golden")
    );
    assert!(String::from_utf8_lossy(&stderr).contains("WriteFailed"));
    assert!(!String::from_utf8_lossy(&stderr).contains(PASSWORD));

    let server = Server::start(delete_replies(http("403 Forbidden", b"{}")));
    let (status, stdout, stderr) = invoke_using(
        &[
            "database",
            "delete-database-instance",
            "--id",
            "db-1",
            "--yes",
        ],
        &server,
        true,
        &mut std::io::empty(),
    );
    assert_eq!(status, ExitCode::FAILURE);
    assert_eq!(
        stdout,
        include_bytes!("fixtures/database/delete-database-instance.partial.json.golden")
    );
    assert!(String::from_utf8_lossy(&stderr).contains("NotSettled"));
    assert!(!String::from_utf8_lossy(&stderr).contains("403"));
}

#[test]
fn database_password_input_refuses_unsafe_sources_before_requests() {
    let server = Server::start(vec![]);
    let secret_json = CREATE_JSON.replacen('{', &format!("{{\"Password\":\"{PASSWORD}\","), 1);
    let oversized = "A".repeat(4097);
    for (json, input, expected) in [
        (secret_json.as_str(), PASSWORD, "secret field"),
        ("file://ignored", PASSWORD, "cannot share input"),
        (CREATE_JSON, "", "non-empty"),
        (CREATE_JSON, "abc\u{1}def\n", "controls"),
        (CREATE_JSON, &oversized, "4096 bytes"),
    ] {
        let mut args = create_args("-", "json");
        args[4] = json;
        let (status, stdout, stderr) = invoke_using(&args, &server, true, &mut input.as_bytes());
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let stderr = String::from_utf8_lossy(&stderr);
        assert!(stderr.contains(expected));
        assert!(!stderr.contains(PASSWORD));
    }
    assert!(server.heads().is_empty());
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
