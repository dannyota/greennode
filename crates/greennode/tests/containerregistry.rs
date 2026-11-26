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
fn incomplete_repository_inventory_prints_no_partial_output() {
    let body = br#"{"data":[{"uuid":"repo-1"}],"totalPage":2,"totalItem":2}"#;
    let server = Server::start(vec![Response::Http {
        status: "200 OK".into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.to_vec(),
    }]);
    let options = with_product_loopback(
        Product::ContainerRegistry,
        ConfigOptions::default().with_static_token(Secret::new("synthetic-cli-registry-token")),
        &server,
    );
    let (context, _) = RequestContext::new();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "containerregistry", "list-repositories"]
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
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("incomplete"));
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

const PERMISSIONS_ARG: &str =
    r#"{"Permissions":[{"RepositoryID":"repo-1","Actions":["Pull Images"]}]}"#;
const EMPTY_USERS: &str = r#"{"data":[],"totalPage":0,"totalItem":0}"#;
const EMPTY_REPOSITORY: &str = r#"{"uuid":"repo-1","imageCount":0}"#;
const USER_SECRET: &str = "synthetic-issued-registry-key";

fn response(status: &str, body: &str) -> Response {
    Response::Http {
        status: status.into(),
        headers: vec![("Content-Length".into(), body.len().to_string())],
        body: body.as_bytes().to_vec(),
    }
}

fn run_write(server: &Server, args: &[&str]) -> (ExitCode, String, String) {
    run_write_with(
        server,
        args,
        ConfigOptions::default().with_static_token(Secret::new("synthetic-cli-registry-token")),
    )
}

fn run_write_with(
    server: &Server,
    args: &[&str],
    config: ConfigOptions,
) -> (ExitCode, String, String) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
    let (context, _) = RequestContext::new();
    let status = runtime.block_on(run_invocation(Invocation {
        args: ["greennode", "containerregistry"]
            .into_iter()
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source: &Source,
        config_options: with_product_loopback(Product::ContainerRegistry, config, server),
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: false,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    (
        status,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

struct LocalSecret(String);
impl LocalSecret {
    fn new() -> Self {
        Self(format!("sc-{:012x}", getrandom::u64().unwrap() >> 16))
    }
}
impl Drop for LocalSecret {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn registry_writes_match_go_goldens() {
    const REPO: &str =
        include_str!("../../greennode-sdk/tests/fixtures/containerregistry/create_repository.json");
    const DELETED: &str =
        include_str!("../../greennode-sdk/tests/fixtures/containerregistry/delete_repository.json");
    const PERMISSIONS: &str =
        include_str!("../../greennode-sdk/tests/fixtures/containerregistry/list_permissions.json");
    let responses = || {
        vec![
            response("200 OK", PERMISSIONS),
            response("200 OK", EMPTY_USERS),
            response("200 OK", &format!(r#"{{"secretKey":"{USER_SECRET}"}}"#)),
            response("200 OK", EMPTY_USERS),
        ]
    };
    let cases = [
        (
            [
                include_str!("fixtures/containerregistry/create-repository.json.golden"),
                include_str!("fixtures/containerregistry/create-repository.table.golden"),
                include_str!("fixtures/containerregistry/create-repository.text.golden"),
            ],
            &[
                "create-repository",
                "--name",
                "app-images",
                "--quota-limit-gb",
                "20",
                "--no-wait",
            ][..],
        ),
        (
            [
                include_str!("fixtures/containerregistry/delete-repository.json.golden"),
                include_str!("fixtures/containerregistry/delete-repository.table.golden"),
                include_str!("fixtures/containerregistry/delete-repository.text.golden"),
            ],
            &[
                "delete-repository",
                "--repository-id",
                "repo-1",
                "--yes",
                "--no-wait",
            ][..],
        ),
        (
            [
                include_str!("fixtures/containerregistry/delete-user.json.golden"),
                include_str!("fixtures/containerregistry/delete-user.table.golden"),
                include_str!("fixtures/containerregistry/delete-user.text.golden"),
            ],
            &["delete-user", "--user-id", "robot-1", "--yes"][..],
        ),
        (
            [
                include_str!("fixtures/containerregistry/create-user.json.golden"),
                include_str!("fixtures/containerregistry/create-user.table.golden"),
                include_str!("fixtures/containerregistry/create-user.text.golden"),
            ],
            &[
                "create-user",
                "--name",
                "app-ci",
                "--query",
                "User.ID",
                "--cli-input-json",
                PERMISSIONS_ARG,
            ][..],
        ),
    ];
    for (goldens, args) in cases {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let (queue, requests) = match args[0] {
                "create-repository" => (vec![response("202 Accepted", REPO)], 1),
                "delete-repository" => (
                    vec![
                        response("200 OK", EMPTY_REPOSITORY),
                        response("202 Accepted", DELETED),
                    ],
                    2,
                ),
                "delete-user" => (vec![response("200 OK", "{}")], 1),
                _ => (responses(), 4),
            };
            let server = Server::start(queue);
            let secret_path = LocalSecret::new();
            let mut args = args.to_vec();
            args.extend(["--output", format]);
            let (expected_status, expected_error) = if args[0] == "create-user" {
                args.extend(["--secret-file", &secret_path.0]);
                (
                    ExitCode::from(1),
                    include_str!("fixtures/containerregistry/create-user.stderr.golden"),
                )
            } else {
                (ExitCode::SUCCESS, "")
            };
            let (status, stdout, stderr) = run_write(&server, &args);
            assert_eq!(status, expected_status, "{} {format}", args[0]);
            assert!(!stdout.contains(USER_SECRET) && !stderr.contains(USER_SECRET));
            assert_eq!(
                stdout,
                golden.replace("registry-secret", &secret_path.0),
                "{} {format}",
                args[0]
            );
            assert_eq!(stderr, expected_error, "{} {format}", args[0]);
            assert_eq!(server.requests(), requests);
            if args[0] == "create-user" {
                let secret = std::fs::read_to_string(&secret_path.0).unwrap();
                assert!(secret.strip_suffix('\n') == Some(USER_SECRET));
            }
        }
    }
}

#[test]
fn guards_refuse_writes_before_any_request() {
    let secret_path = LocalSecret::new();
    let server = Server::start(vec![]);
    for args in [
        &[
            "create-repository",
            "--name",
            "app-images",
            "--quota-limit-gb",
            "20",
            "--read-only",
        ][..],
        &[
            "delete-repository",
            "--repository-id",
            "repo-1",
            "--yes",
            "--read-only",
        ][..],
        &[
            "delete-user",
            "--user-id",
            "robot-1",
            "--yes",
            "--read-only",
        ][..],
        &[
            "create-user",
            "--name",
            "app-ci",
            "--cli-input-json",
            PERMISSIONS_ARG,
            "--secret-file",
            &secret_path.0,
            "--read-only",
        ][..],
    ] {
        let (status, stdout, stderr) = run_write(&server, args);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(stderr.contains("ReadOnly"));
    }
    for args in [
        &["delete-repository", "--repository-id", "repo-1"][..],
        &["delete-user", "--user-id", "robot-1"][..],
        &[
            "create-user",
            "--name",
            "app-ci",
            "--cli-input-json",
            PERMISSIONS_ARG,
        ][..],
    ] {
        let (status, stdout, stderr) = run_write(&server, args);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(stderr.contains("InvalidUsage"));
    }
    assert_eq!(server.requests(), 0);
    assert!(!std::path::Path::new(&secret_path.0).exists());
}

#[cfg(unix)]
fn create_user_responses(secret: &str) -> Vec<Response> {
    vec![
        response(
            "200 OK",
            include_str!(
                "../../greennode-sdk/tests/fixtures/containerregistry/list_permissions.json"
            ),
        ),
        response("200 OK", EMPTY_USERS),
        response(
            "200 OK",
            &serde_json::json!({"secretKey": secret}).to_string(),
        ),
        response("200 OK", EMPTY_USERS),
    ]
}

#[cfg(unix)]
fn user_args(path: &str) -> [&str; 7] {
    [
        "create-user",
        "--name",
        "app-ci",
        "--cli-input-json",
        PERMISSIONS_ARG,
        "--secret-file",
        path,
    ]
}

#[cfg(unix)]
#[test]
fn create_user_masks_trimmed_and_escaped_secret_filenames() {
    for spelling in [USER_SECRET, "synthetic-issued-registry-\\u006bey"] {
        let path = LocalSecret(format!(
            "sc-{:012x}-{spelling}",
            getrandom::u64().unwrap() >> 16
        ));
        let secret = format!("{USER_SECRET}\n");
        let server = Server::start(create_user_responses(&secret));
        let (status, stdout, stderr) = run_write(&server, &user_args(&path.0));
        assert_eq!(status, ExitCode::from(1));
        assert!(stderr.contains("UserNotFound"));
        assert!(!stdout.contains(spelling));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stdout).unwrap()["SecretFile"],
            "<redacted>"
        );
        assert!(std::fs::read_to_string(&path.0).unwrap() == secret);
    }
}

#[cfg(unix)]
#[test]
fn create_user_masks_a_short_trimmed_issued_key_in_the_filename() {
    for spelling in ["toy-key", "toy-%6bey"] {
        let path = LocalSecret(format!(
            "sc-{:012x}-{spelling}",
            getrandom::u64().unwrap() >> 16
        ));
        let server = Server::start(create_user_responses("toy-key\n"));
        let (status, stdout, stderr) = run_write(&server, &user_args(&path.0));
        assert_eq!(status, ExitCode::from(1));
        assert!(stderr.contains("UserNotFound"));
        let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(output["SecretFile"].as_str() == Some("<redacted>"));
        assert!(std::fs::read(&path.0).unwrap() == b"toy-key\n");
    }
}

#[cfg(unix)]
#[derive(Default)]
struct FailingCandidateAuth(std::sync::atomic::AtomicUsize);

#[cfg(unix)]
impl greennode_sdk::CredentialsProvider for FailingCandidateAuth {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<greennode_sdk::Token, greennode_sdk::Error>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 3 {
                Err(greennode_sdk::Error::provider_failed())
            } else {
                Ok(greennode_sdk::Token::new(
                    Secret::new("synthetic-cli-registry-token"),
                    None,
                ))
            }
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), greennode_sdk::Error>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(unix)]
#[test]
fn create_user_saves_issued_secret_if_lookup_authentication_fails() {
    let path = LocalSecret::new();
    let server = Server::start(create_user_responses(USER_SECRET));
    let config = ConfigOptions::default()
        .with_credentials_provider(std::sync::Arc::new(FailingCandidateAuth::default()));
    let (status, stdout, stderr) = run_write_with(&server, &user_args(&path.0), config);
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(stderr.contains("UserNotFound"));
    assert!(!stderr.contains(USER_SECRET));
    assert_eq!(server.requests(), 3);
    assert!(
        std::fs::read_to_string(&path.0)
            .is_ok_and(|secret| secret.strip_suffix('\n') == Some(USER_SECRET))
    );
}
