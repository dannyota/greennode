use super::{BEARER, invoke, response};
use greennode_sdk::testing::{Response, Server};
use std::process::ExitCode;

fn reply(status: u16, json: &str) -> Response {
    response(status, json.as_bytes().to_vec())
}

const CALLER: &str = r#"{"userId":"caller-1","userType":"iam-user"}"#;
const USER: &str = r#"{"id":"user-1","username":"worker","from":"iam","twoFactorAuth":{"googleAuthenticator":{"verified":true,"enabled":false,"secret":"toy-factor-seed"}}}"#;
const ENABLED: &str = r#"{"id":"user-1","username":"worker","from":"iam","twoFactorAuth":{"googleAuthenticator":{"verified":true,"enabled":true,"recoveryCodes":["toy-recovery-code"]}}}"#;

fn admin_guard(user: &str) -> Vec<Response> {
    vec![
        reply(200, CALLER),
        reply(200, user),
        reply(200, r#"[{"action":"CreatePolicy","label":"Write"}]"#),
        reply(200, r#"{"data":[],"totalItems":0}"#),
        reply(200, "[]"),
    ]
}

#[tokio::test]
async fn delete_user_checks_caller_and_origin_then_deletes() {
    let server = Server::start(vec![reply(200, CALLER), reply(200, USER), reply(204, "")]);
    let (code, stdout, stderr) = invoke(
        &["iam", "delete-user", "--user-id", "user-1", "--yes"],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(stdout, b"{}\n");
    assert!(stderr.is_empty());
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    for (head, route) in heads.iter().zip([
        "GET /dashboard/accounts-api/v1/auth/userinfo ",
        "GET /dashboard/accounts-api/v1/iam-users/user-1 ",
        "DELETE /dashboard/accounts-api/v1/iam-users/user-1 ",
    ]) {
        assert!(head.starts_with(route.as_bytes()));
    }
}

#[tokio::test]
async fn google_mfa_toggles_a_verified_factor_and_confirms_once() {
    for (command, suffix, before, after, enabled) in [
        ("enable-user-mfa", "activate-google", USER, ENABLED, true),
        (
            "disable-user-mfa",
            "deactivate-google",
            ENABLED,
            USER,
            false,
        ),
    ] {
        let mut replies = admin_guard(before);
        replies.extend([reply(204, ""), reply(200, after)]);
        let server = Server::start(replies);
        let (code, stdout, stderr) = invoke(
            &[
                "iam",
                command,
                "--user-id",
                "user-1",
                "--method",
                "google",
                "--yes",
            ],
            &server,
            BEARER,
        )
        .await;
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let output: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(
            output,
            serde_json::json!({"Method":"google", "Enabled":enabled, "Changed":true})
        );
        let heads = server.heads();
        assert_eq!(heads.len(), 7);
        assert!(heads[2].starts_with(b"GET /iam/policies-api/v1/actions?product=iam "));
        assert!(
            heads[3].starts_with(
                b"GET /iam/policies-api/v1/user-attachments/iam-users/user-1/policies?"
            )
        );
        assert!(
            heads[4]
                .starts_with(b"GET /iam/policies-api/v1/user-attachments/iam-users/user-1/groups ")
        );
        assert!(heads[5].starts_with(
            format!("POST /dashboard/accounts-api/v1/iam-users/user-1/{suffix} ").as_bytes()
        ));
        assert!(heads[6].starts_with(b"GET /dashboard/accounts-api/v1/iam-users/user-1 "));
        assert!(server.bodies()[5].is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn create_and_reset_send_only_the_password_from_the_private_file() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let file =
        std::env::temp_dir().join(format!("greennode-iam-input-{}", getrandom::u64().unwrap()));
    let mut input = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .unwrap();
    input.write_all(b"private-toy\n").unwrap();
    drop(input);
    let filename = file.to_str().unwrap();
    let server = Server::start(vec![reply(201, r#"{"id":"user-1"}"#), reply(200, USER)]);
    let create = invoke(
        &[
            "iam",
            "create-user",
            "--username",
            "worker",
            "--password-file",
            filename,
        ],
        &server,
        BEARER,
    )
    .await;
    let mut replies = admin_guard(USER);
    replies.push(reply(204, ""));
    let reset_server = Server::start(replies);
    let reset = invoke(
        &[
            "iam",
            "reset-user-password",
            "--user-id",
            "user-1",
            "--password-file",
            filename,
            "--yes",
        ],
        &reset_server,
        BEARER,
    )
    .await;
    std::fs::remove_file(file).unwrap();
    for (code, _, stderr) in [&create, &reset] {
        assert_eq!(*code, ExitCode::SUCCESS);
        assert!(stderr.is_empty());
    }
    assert_eq!(reset.1, b"{}\n");
    assert!(server.bodies().first().is_some_and(|bytes| {
        serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|body| {
            body == serde_json::json!({"username":"worker", "password":"private-toy"})
        })
    }));
    assert!(reset_server.bodies().get(5).is_some_and(|bytes| {
        serde_json::from_slice::<serde_json::Value>(bytes)
            .is_ok_and(|body| body == serde_json::json!({"password":"private-toy"}))
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn user_writes_match_go_goldens_and_keep_generated_passwords_private() {
    use super::fixture;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        args: Vec<String>,
        exit: u8,
        replies: Vec<Reply>,
    }
    #[derive(serde::Deserialize)]
    struct Reply {
        status: u16,
        body: serde_json::Value,
    }
    let cases: Vec<Case> = serde_json::from_slice(&fixture("user-writes/cases.json")).unwrap();
    for case in cases {
        for format in ["json", "table", "text"] {
            let directory = PrivateDir::new();
            let secret = directory.0.join("issued");
            let password = directory.0.join("input");
            std::io::Write::write_all(
                &mut std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&password)
                    .unwrap(),
                b"private-toy\n",
            )
            .unwrap();
            let secret_path = secret.to_str().unwrap();
            let password_path = password.to_str().unwrap();
            let server = Server::start(
                case.replies
                    .iter()
                    .map(|reply| {
                        let body = if reply.status == 204 {
                            Vec::new()
                        } else {
                            serde_json::to_vec(&reply.body).unwrap()
                        };
                        response(reply.status, body)
                    })
                    .collect(),
            );
            let mut args = vec!["--output", format, "iam"];
            args.extend(case.args.iter().map(|arg| match arg.as_str() {
                "$SECRET" => secret_path,
                "$PASSWORD" => password_path,
                value => value,
            }));
            let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
            assert_eq!(exit, ExitCode::from(case.exit), "{} {format}", case.name);
            assert!(stderr.is_empty());
            assert_eq!(server.requests(), case.replies.len());
            let expected = String::from_utf8(fixture(&format!(
                "user-writes/{}.{format}.golden",
                case.name
            )))
            .unwrap();
            let stdout = String::from_utf8(stdout)
                .unwrap()
                .replace(secret_path, "$SECRET");
            if args.contains(&"--secret-file") {
                let contents = std::fs::read_to_string(&secret).unwrap();
                let password = contents.strip_suffix('\n').unwrap();
                assert!(
                    password.len() == 32
                        && password.contains('@')
                        && password.bytes().any(|b| b.is_ascii_uppercase())
                        && password.bytes().any(|b| b.is_ascii_lowercase())
                        && password.bytes().any(|b| b.is_ascii_digit())
                );
                assert!(!stdout.contains(password));
                assert_eq!(
                    std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                if format == "table" {
                    assert_eq!(table_cells(&stdout), table_cells(&expected));
                    continue;
                }
            }
            assert_eq!(stdout, expected, "{} {format}", case.name);
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
            std::env::temp_dir().join(format!("greennode-iam-user-{}", getrandom::u64().unwrap()));
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

#[tokio::test]
async fn user_write_guards_refuse_self_non_iam_and_unusable_mfa() {
    let cases = [
        (
            vec![reply(200, CALLER)],
            vec!["iam", "delete-user", "--user-id", "caller-1", "--yes"],
            "SelfChange",
        ),
        (
            vec![
                reply(200, CALLER),
                reply(200, r#"{"id":"user-1","from":"idp"}"#),
            ],
            vec!["iam", "delete-user", "--user-id", "user-1", "--yes"],
            "RequestFailed",
        ),
        (
            admin_guard(
                r#"{"id":"user-1","from":"iam","twoFactorAuth":{"googleAuthenticator":{"verified":false,"enabled":false}}}"#,
            ),
            vec![
                "iam",
                "enable-user-mfa",
                "--user-id",
                "user-1",
                "--yes",
                "--method",
                "google",
            ],
            "MFA state is unavailable or unverified",
        ),
    ];
    for (replies, args, expected) in cases {
        let requests = replies.len();
        let server = Server::start(replies);
        let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
        assert_eq!(exit, ExitCode::from(1));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains(expected));
        assert_eq!(server.requests(), requests);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn uncertain_create_delivers_generated_password_without_printing_a_partial() {
    let tmp = PrivateDir::new();
    let file = tmp.0.join("generated");
    let server = Server::start(vec![reply(500, r#"{"message":"private-toy"}"#)]);
    let (exit, stdout, stderr) = invoke(
        &[
            "iam",
            "create-user",
            "--username",
            "worker",
            "--secret-file",
            file.to_str().unwrap(),
        ],
        &server,
        BEARER,
    )
    .await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    let password = std::fs::read_to_string(file).unwrap();
    let password = password.strip_suffix('\n').unwrap();
    let message = String::from_utf8_lossy(&stderr);
    assert_eq!(password.len(), 32);
    assert!(!message.contains(password));
    assert!(
        message.contains("RequestFailed") && message.contains("password ownership is unproven")
    );
    assert_eq!(server.requests(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_create_does_not_save_password_when_status_is_withheld() {
    let tmp = PrivateDir::new();
    let file = tmp.0.join("generated");
    let server = Server::start(vec![reply(404, "{}")]);
    let (exit, stdout, stderr) = invoke(
        &[
            "iam",
            "create-user",
            "--username",
            "worker",
            "--secret-file",
            file.to_str().unwrap(),
        ],
        &server,
        "404",
    )
    .await;
    assert_eq!(exit, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
    assert!(error["error"].get("status").is_none());
    assert!(!file.exists());
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn mfa_confirmation_wrong_user_or_origin_is_not_settled() {
    for confirmation in [
        r#"{"id":"wrong-user","from":"iam"}"#,
        r#"{"id":"user-1","from":"idp"}"#,
    ] {
        let mut replies = admin_guard(USER);
        replies.extend([reply(204, ""), reply(200, confirmation)]);
        let server = Server::start(replies);
        let (exit, stdout, stderr) = invoke(
            &[
                "iam",
                "enable-user-mfa",
                "--user-id",
                "user-1",
                "--method",
                "google",
                "--yes",
            ],
            &server,
            BEARER,
        )
        .await;
        assert_eq!(exit, ExitCode::from(1));
        assert!(stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "NotSettled");
        assert_eq!(server.requests(), 7);
    }
}
