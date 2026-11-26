use greennode::cli::{Invocation, run_invocation};
use greennode_sdk::testing::{Product, Response, Server, with_product_loopback};
use greennode_sdk::{ConfigOptions, ConfigSource, RequestContext, Secret, SystemClock};
use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

struct Source;
impl ConfigSource for Source {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        (name == "GREENNODE_REGION").then(|| "hcm-3".into())
    }
}

#[test]
fn registry_reads_match_go_goldens() {
    type Case<'a> = (&'a str, &'a [&'a str], &'a [u8], [&'a [u8]; 3]);
    let cases: &[Case<'_>] = &[
        (
            "list-repositories",
            &[],
            include_bytes!(
                "../../greennode-sdk/tests/fixtures/containerregistry/list_repositories.json"
            ),
            [
                include_bytes!("fixtures/containerregistry/list-repositories.json.golden"),
                include_bytes!("fixtures/containerregistry/list-repositories.table.golden"),
                include_bytes!("fixtures/containerregistry/list-repositories.text.golden"),
            ],
        ),
        (
            "get-repository",
            &["--repository-id", "repo-1"],
            include_bytes!(
                "../../greennode-sdk/tests/fixtures/containerregistry/get_repository.json"
            ),
            [
                include_bytes!("fixtures/containerregistry/get-repository.json.golden"),
                include_bytes!("fixtures/containerregistry/get-repository.table.golden"),
                include_bytes!("fixtures/containerregistry/get-repository.text.golden"),
            ],
        ),
        (
            "list-users",
            &[],
            include_bytes!("../../greennode-sdk/tests/fixtures/containerregistry/list_users.json"),
            [
                include_bytes!("fixtures/containerregistry/list-users.json.golden"),
                include_bytes!("fixtures/containerregistry/list-users.table.golden"),
                include_bytes!("fixtures/containerregistry/list-users.text.golden"),
            ],
        ),
        (
            "list-repository-users",
            &["--repository-id", "repo-1"],
            include_bytes!("../../greennode-sdk/tests/fixtures/containerregistry/list_users.json"),
            [
                include_bytes!("fixtures/containerregistry/list-repository-users.json.golden"),
                include_bytes!("fixtures/containerregistry/list-repository-users.table.golden"),
                include_bytes!("fixtures/containerregistry/list-repository-users.text.golden"),
            ],
        ),
        (
            "list-permissions",
            &[],
            include_bytes!(
                "../../greennode-sdk/tests/fixtures/containerregistry/list_permissions.json"
            ),
            [
                include_bytes!("fixtures/containerregistry/list-permissions.json.golden"),
                include_bytes!("fixtures/containerregistry/list-permissions.table.golden"),
                include_bytes!("fixtures/containerregistry/list-permissions.text.golden"),
            ],
        ),
    ];
    for (command, flags, body, expected) in cases {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(expected) {
            let server = Server::start(vec![Response::Http {
                status: "200 OK".into(),
                headers: vec![("Content-Length".into(), body.len().to_string())],
                body: body.to_vec(),
            }]);
            let (context, _) = RequestContext::new();
            let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
            let options = with_product_loopback(
                Product::ContainerRegistry,
                ConfigOptions::default()
                    .with_static_token(Secret::new("synthetic-cli-registry-token")),
                &server,
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let args = [
                "greennode",
                "--output",
                format,
                "containerregistry",
                command,
            ]
            .into_iter()
            .chain(flags.iter().copied())
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
                "{command}/{format}: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert_eq!(&stdout, golden, "{command}/{format}");
            assert!(stderr.is_empty());
            assert_eq!(server.requests(), 1);
        }
    }
}

#[test]
fn registry_read_is_registered() {
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "containerregistry", "list-permissions"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: ConfigOptions::default(),
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    assert_eq!(result, ExitCode::from(3));
}
