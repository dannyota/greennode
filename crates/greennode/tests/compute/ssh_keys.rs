use super::*;
use greennode_sdk::{CredentialsProvider, Error, Token};
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const ISSUED: &str = "toy-private-key";
const CREATED: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/ssh-key-created.json");
const IMPORTED: &str =
    include_str!("../../../greennode-sdk/tests/fixtures/compute/ssh-key-imported.json");

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let name = format!(
                "greennode-ssh-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn ssh_writes_match_go_goldens_and_deliver_the_private_key_once() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (command, route, status, body, arguments) in [
        (
            "create-ssh-key",
            "POST /v2/proj-1/sshKeys ",
            "201 Created",
            CREATED,
            &["--name", "example"][..],
        ),
        (
            "import-ssh-key",
            "POST /v2/proj-1/sshKeys/import ",
            "201 Created",
            IMPORTED,
            &["--name", "example", "--public-key", "ssh-rsa toy public"],
        ),
        (
            "delete-ssh-key",
            "DELETE /v2/proj-1/sshKeys/key-1 ",
            "204 No Content",
            "",
            &["--ssh-key-id", "key-1", "--yes"],
        ),
    ] {
        for format in ["json", "table", "text"] {
            let directory = TestDirectory::new();
            let path = directory.0.join("toy.key");
            let server = Server::start(vec![response(status, body)]);
            let mut args = vec![command, "--project-id", "proj-1", "--output", format];
            args.extend_from_slice(arguments);
            if command == "create-ssh-key" {
                args.extend(["--secret-file", path.to_str().unwrap(), "--query", "{SSHKey: SSHKey, PrivateKey: PrivateKey, SecretFile: contains(SecretFile, 'toy.key')}"]);
            }
            let (exit, stdout, stderr) = invoke(&server, &args, false).await;
            assert_eq!(exit, ExitCode::SUCCESS, "{command}/{format}");
            assert!(!String::from_utf8_lossy(&stdout).contains(ISSUED));
            assert!(stderr.is_empty());
            let golden = root.join(format!("tests/fixtures/compute/{command}.{format}.golden"));
            assert_eq!(stdout, std::fs::read(golden).unwrap(), "{command}/{format}");
            let heads = server.heads();
            assert_eq!(heads.len(), 1);
            assert!(heads[0].starts_with(route.as_bytes()));
            if command == "create-ssh-key" {
                assert!(std::fs::read_to_string(&path).unwrap() == format!("{ISSUED}\n"));
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
}

#[tokio::test]
async fn ssh_guards_refuse_existing_destinations_and_unconfirmed_deletes() {
    let directory = TestDirectory::new();
    let path = directory.0.join("present.key");
    std::fs::write(&path, "keep").unwrap();
    let server = Server::start(vec![]);
    for args in [
        &[
            "create-ssh-key",
            "--name",
            "example",
            "--secret-file",
            path.to_str().unwrap(),
        ][..],
        &["create-ssh-key", "--name", "example"],
        &["delete-ssh-key", "--ssh-key-id", "key-1"],
        &[
            "delete-ssh-key",
            "--ssh-key-id",
            "key-1",
            "--yes",
            "--read-only",
        ],
    ] {
        let (exit, stdout, _) = invoke(&server, args, false).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
    }
    assert!(std::fs::read_to_string(path).unwrap() == "keep");
    assert_eq!(server.requests(), 0);
}

struct RaceDestination {
    path: PathBuf,
    wrote: AtomicBool,
}

impl CredentialsProvider for RaceDestination {
    fn token<'a>(
        &'a self,
        _: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + 'a>> {
        Box::pin(async move {
            if !self.wrote.swap(true, Ordering::SeqCst) {
                std::fs::write(&self.path, "keep").unwrap();
            }
            Ok(Token::new(Secret::new(BEARER), None))
        })
    }

    fn invalidate<'a>(
        &'a self,
        _: &'a RequestContext,
        _: &'a Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn failed_secret_delivery_cleans_up_only_the_issued_key_without_printing_it() {
    let directory = TestDirectory::new();
    let path = directory.0.join("toy.key");
    let server = Server::start(vec![
        response("201 Created", CREATED),
        response("204 No Content", ""),
    ]);
    let options = ConfigOptions::default().with_credentials_provider(Arc::new(RaceDestination {
        path: path.clone(),
        wrote: AtomicBool::new(false),
    }));
    let args = [
        "create-ssh-key",
        "--project-id",
        "proj-1",
        "--name",
        "example",
        "--secret-file",
        path.to_str().unwrap(),
    ];
    let (exit, stdout, stderr) = invoke_with_options(&server, &args, false, options).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(!String::from_utf8_lossy(&stderr).contains(ISSUED));
    let diagnostic: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], "SecretFileFailed");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains("key-1 was deleted")
    );
    assert!(std::fs::read_to_string(&path).unwrap() == "keep");
    let heads = server.heads();
    assert_eq!(heads.len(), 2);
    assert!(heads[0].starts_with(b"POST /v2/proj-1/sshKeys "));
    assert!(heads[1].starts_with(b"DELETE /v2/proj-1/sshKeys/key-1 "));
}

#[tokio::test]
async fn ssh_secret_file_path_is_screened_before_projection() {
    let directory = TestDirectory::new();
    let path = directory.0.join(ISSUED);
    let server = Server::start(vec![response("201 Created", CREATED)]);
    let (exit, stdout, stderr) = invoke(
        &server,
        &[
            "create-ssh-key",
            "--project-id",
            "proj-1",
            "--name",
            "example",
            "--secret-file",
            path.to_str().unwrap(),
        ],
        false,
    )
    .await;
    assert_eq!(exit, ExitCode::SUCCESS);
    assert!(stderr.is_empty());
    assert!(!String::from_utf8_lossy(&stdout).contains(ISSUED));
    let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(output["SecretFile"], "<redacted>");
    assert!(std::fs::read_to_string(path).unwrap() == format!("{ISSUED}\n"));
}

#[tokio::test]
async fn create_rejects_reflected_bearer_in_an_unknown_reply_field_before_delivery() {
    let directory = TestDirectory::new();
    let path = directory.0.join("toy.key");
    let raw =
        serde_json::json!({"data":{"id":"key-1","privateKey":ISSUED},"unknown":{"session":BEARER}})
            .to_string();
    let server = Server::start(vec![response("201 Created", &raw)]);
    let args = [
        "create-ssh-key",
        "--project-id",
        "proj-1",
        "--name",
        "example",
        "--secret-file",
        path.to_str().unwrap(),
    ];
    let (exit, stdout, stderr) = invoke(&server, &args, false).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(!path.exists());
    assert!(!String::from_utf8_lossy(&stderr).contains(ISSUED));
    let diagnostic: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], "InvalidResponse");
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn issued_key_hex_reflection_is_never_used_for_cleanup() {
    let directory = TestDirectory::new();
    let path = directory.0.join("toy.key");
    let hex: String = ISSUED.bytes().map(|byte| format!("{byte:02X}")).collect();
    let raw = serde_json::json!({"data":{"id":hex,"privateKey":ISSUED}}).to_string();
    let server = Server::start(vec![
        response("201 Created", &raw),
        response("204 No Content", ""),
    ]);
    let options = ConfigOptions::default().with_credentials_provider(Arc::new(RaceDestination {
        path: path.clone(),
        wrote: AtomicBool::new(false),
    }));
    let args = [
        "create-ssh-key",
        "--project-id",
        "proj-1",
        "--name",
        "example",
        "--secret-file",
        path.to_str().unwrap(),
    ];
    let (exit, stdout, stderr) = invoke_with_options(&server, &args, false, options).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(!String::from_utf8_lossy(&stderr).contains(&hex));
    assert!(!String::from_utf8_lossy(&stderr).contains(ISSUED));
    assert_eq!(server.requests(), 1);
    assert!(std::fs::read_to_string(path).unwrap() == "keep");
}
