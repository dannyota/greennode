use super::{BEARER, invoke, response};
use greennode_sdk::testing::{Response, Server};
use std::process::ExitCode;

#[cfg(unix)]
use super::fixture;

#[cfg(unix)]
#[derive(serde::Deserialize)]
struct GoldenCase {
    name: String,
    args: Vec<String>,
    exit: u8,
    #[serde(default)]
    guard: bool,
    replies: Vec<GoldenReply>,
}

#[cfg(unix)]
#[derive(serde::Deserialize)]
struct GoldenReply {
    status: u16,
    body: serde_json::Value,
}

#[cfg(unix)]
#[tokio::test]
async fn service_account_writes_match_go_goldens() {
    use std::os::unix::fs::PermissionsExt;

    let cases: Vec<GoldenCase> =
        serde_json::from_slice(&fixture("service-account-writes/cases.json")).unwrap();
    for case in cases {
        let formats: &[&str] = if case.exit == 0 {
            &["json", "text", "table"]
        } else {
            &["json"]
        };
        for format in formats {
            let directory = PrivateDir::new();
            let file = directory.0.join("issued");
            let file_name = file.to_str().unwrap();
            let mut replies = if case.guard {
                guard_responses()
            } else {
                Vec::new()
            };
            replies.extend(case.replies.iter().map(|reply| {
                let body = if reply.status == 204 {
                    Vec::new()
                } else {
                    serde_json::to_vec(&reply.body).unwrap()
                };
                response(reply.status, body)
            }));
            let expected_requests = replies.len();
            let server = Server::start(replies);
            let mut args = vec!["--output", format, "iam"];
            args.extend(
                case.args
                    .iter()
                    .map(|arg| if arg == "$SECRET" { file_name } else { arg }),
            );
            let (status, stdout, stderr) = invoke(&args, &server, BEARER).await;
            assert_eq!(status, ExitCode::from(case.exit), "{} {format}", case.name);
            assert_eq!(
                server.requests(),
                expected_requests,
                "{} {format}",
                case.name
            );
            let expected = fixture(&format!(
                "service-account-writes/{}.{format}.golden",
                case.name
            ));
            let expected = String::from_utf8(expected).unwrap();
            let stdout = String::from_utf8(stdout)
                .unwrap()
                .replace(file_name, "$SECRET");
            if *format == "table" && args.contains(&"--secret-file") {
                assert_eq!(
                    table_cells(&stdout),
                    table_cells(&expected),
                    "{} {format}",
                    case.name
                );
            } else {
                assert_eq!(stdout, expected, "{} {format}", case.name);
            }
            if case.exit == 0 {
                assert!(stderr.is_empty());
            } else {
                let expected = fixture(&format!(
                    "service-account-writes/{}.stderr.golden",
                    case.name
                ));
                assert_eq!(
                    String::from_utf8(stderr)
                        .unwrap()
                        .replace(file_name, "$SECRET"),
                    String::from_utf8(expected).unwrap(),
                    "{}",
                    case.name
                );
            }
            if args.contains(&"--secret-file") && !case.name.ends_with("missing-secret") {
                assert!(std::fs::read(&file).is_ok_and(|data| data == b"private-toy\n"));
                assert_eq!(
                    std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
}

#[cfg(unix)]
fn table_cells(table: &str) -> Vec<Vec<&str>> {
    table
        .lines()
        .filter(|line| line.starts_with('|'))
        .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
        .collect()
}

#[cfg(unix)]
struct PrivateDir(std::path::PathBuf);

#[cfg(unix)]
impl PrivateDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("greennode-iam-{}", getrandom::u64().unwrap()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

#[cfg(unix)]
impl Drop for PrivateDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn reply(status: u16, body: &str) -> Response {
    response(status, body.as_bytes().to_vec())
}

fn guard_responses() -> Vec<Response> {
    vec![
        reply(200, r#"{"userId":"user-1","userType":"iam-user"}"#),
        reply(200, r#"[{"action":"CreatePolicy","label":"Write"}]"#),
        reply(200, r#"{"data":[],"totalItems":0}"#),
    ]
}

#[tokio::test]
async fn delete_service_account_checks_guard_then_deletes() {
    let mut replies = guard_responses();
    replies.push(reply(204, ""));
    let server = Server::start(replies);
    let args = [
        "iam",
        "delete-service-account",
        "--service-account-id",
        "sa-1",
        "--yes",
    ];
    let (status, stdout, stderr) = invoke(&args, &server, BEARER).await;
    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(stdout, b"{}\n");
    assert!(stderr.is_empty());
    let heads = server.heads();
    for (head, route) in heads.iter().zip([
        "GET /dashboard/accounts-api/v1/auth/userinfo ",
        "GET /iam/policies-api/v1/actions?product=iam ",
        "GET /iam/policies-api/v1/user-attachments/service-accounts/sa-1/policies?pageNumber=0&pageSize=10000 ",
        "DELETE /dashboard/accounts-api/v1/service-accounts/sa-1 ",
    ]) {
        assert!(head.starts_with(route.as_bytes()));
    }
    assert_eq!(heads.len(), 4);
}

#[tokio::test]
async fn classified_service_account_caller_cannot_delete_a_service_account() {
    let server = Server::start(vec![reply(
        200,
        r#"{"userId":"caller","userType":"user-sa"}"#,
    )]);
    let (status, stdout, stderr) = invoke(
        &[
            "iam",
            "delete-service-account",
            "--service-account-id",
            "sa-1",
            "--yes",
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("SelfChange"));
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn update_service_account_checks_guard_and_reads_back() {
    let mut replies = guard_responses();
    replies.extend([
        reply(200, ""),
        reply(200, r#"{"id":"sa-1","description":"new"}"#),
    ]);
    let server = Server::start(replies);
    let (status, stdout, stderr) = invoke(
        &[
            "iam",
            "update-service-account",
            "--service-account-id",
            "sa-1",
            "--description",
            "new",
            "--access-token-life-span",
            "0",
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(status, ExitCode::SUCCESS);
    assert!(String::from_utf8_lossy(&stdout).contains(r#""Description": "new""#));
    assert!(stderr.is_empty());
    let heads = server.heads();
    assert_eq!(heads.len(), 5);
    assert!(heads[3].starts_with(b"PATCH /dashboard/accounts-api/v1/service-accounts/sa-1 "));
    assert!(heads[4].starts_with(b"GET /dashboard/accounts-api/v1/service-accounts/sa-1 "));
    let bodies = server.bodies();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bodies[3]).is_ok_and(|body| {
            body == serde_json::json!({"description":"new", "accessTokenLifeSpan":0})
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn create_service_account_delivers_unconfirmed_secret_privately() {
    let directory = PrivateDir::new();
    let file = directory.0.join("issued");
    let server = Server::start(vec![reply(
        201,
        r#"{"id":"private-toy","clientSecret":"private-toy"}"#,
    )]);
    let (status, stdout, stderr) = invoke(
        &[
            "iam",
            "create-service-account",
            "--name",
            "new",
            "--secret-file",
            file.to_str().unwrap(),
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("RequestFailed"));
    assert!(std::fs::read(&file).is_ok_and(|data| data == b"private-toy\n"));
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn reset_service_account_missing_secret_has_dedicated_error() {
    let directory = PrivateDir::new();
    let file = directory.0.join("issued");
    let mut replies = guard_responses();
    replies.push(reply(200, "{}"));
    let server = Server::start(replies);
    let (status, stdout, stderr) = invoke(
        &[
            "iam",
            "reset-service-account-secret",
            "--service-account-id",
            "sa-1",
            "--secret-file",
            file.to_str().unwrap(),
            "--yes",
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("SecretFileFailed"));
    assert!(!file.exists());
    assert_eq!(server.requests(), 4);
}

#[cfg(unix)]
#[tokio::test]
async fn create_without_secret_and_failed_confirmation_reports_secret_file_failure() {
    let directory = PrivateDir::new();
    let file = directory.0.join("issued");
    let server = Server::start(vec![reply(201, r#"{"id":"sa-1"}"#), reply(404, "{}")]);
    let (status, stdout, stderr) = invoke(
        &[
            "iam",
            "create-service-account",
            "--name",
            "new",
            "--secret-file",
            file.to_str().unwrap(),
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(status, ExitCode::from(1));
    assert!(stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stderr).unwrap(),
        serde_json::json!({"error": {
            "code": "SecretFileFailed",
            "message": "service account sa-1 was created but the response held no client secret; no file was written; run reset-service-account-secret to get one",
        }}),
    );
    assert!(!file.exists());
    assert_eq!(server.requests(), 2);
}
