use super::{BEARER, fixture, invoke, response};
use greennode_sdk::testing::{Response, Server};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::ExitCode;

#[derive(Deserialize)]
struct Case {
    name: String,
    args: Vec<String>,
    exit: u8,
    replies: Vec<Reply>,
}

#[derive(Deserialize)]
struct Reply {
    method: String,
    route: String,
    status: u16,
    #[serde(default)]
    body: Value,
    #[serde(rename = "rawBody")]
    raw_body: Option<String>,
    request: Option<Value>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

#[test]
fn account_policy_docs_name_both_guard_refusals() {
    let docs = include_str!("../../../../docs/wiki/CLI-IAM.md");
    for operation in [
        "attach-service-account-policy",
        "detach-service-account-policy",
    ] {
        let section = docs
            .split(&format!("## {operation}\n"))
            .nth(1)
            .unwrap()
            .split("\n## ")
            .next()
            .unwrap();
        assert!(section.contains("`SelfChange`"));
        assert!(section.contains("`PrivilegedChange`"));
    }
}

#[tokio::test]
async fn writes_match_go_routes_exit_codes_and_output() {
    check_go_goldens("writes", |case| {
        !matches!(
            case,
            "create-group-invalid-path-id"
                | "create-group-redirect"
                | "create-group-malformed-confirmation"
        )
    })
    .await;
}

#[tokio::test]
async fn create_group_retains_go_redirect_status() {
    check_go_goldens("writes", |case| case == "create-group-redirect").await;
}

#[tokio::test]
async fn create_group_malformed_confirmation_matches_go() {
    check_go_goldens("writes", |case| {
        case == "create-group-malformed-confirmation"
    })
    .await;
}

#[tokio::test]
async fn create_group_path_invalid_id_recovery_matches_go() {
    check_go_goldens("writes", |case| case == "create-group-invalid-path-id").await;
}

#[tokio::test]
async fn policy_writes_match_go_routes_exit_codes_and_output() {
    check_go_goldens("policy-writes", |_| true).await;
}

async fn check_go_goldens(dir: &str, select: impl Fn(&str) -> bool) {
    let cases: Vec<Case> = serde_json::from_slice(&fixture(&format!("{dir}/cases.json"))).unwrap();
    for case in cases.into_iter().filter(|case| select(&case.name)) {
        let formats: &[&str] = if case.exit == 0 {
            &["json", "table", "text"]
        } else {
            &["json"]
        };
        for format in formats {
            let server = Server::start(
                case.replies
                    .iter()
                    .map(|reply| {
                        let body = if reply.status == 204 {
                            Vec::new()
                        } else if let Some(raw_body) = &reply.raw_body {
                            raw_body.as_bytes().to_vec()
                        } else {
                            serde_json::to_vec(&reply.body).unwrap()
                        };
                        let mut reply_response = response(reply.status, body);
                        if let Response::Http { headers, .. } = &mut reply_response {
                            headers.extend(reply.headers.clone());
                        }
                        reply_response
                    })
                    .collect(),
            );
            let mut args = vec!["--output", format, "iam"];
            args.extend(case.args.iter().map(String::as_str));
            let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
            assert_eq!(
                exit,
                ExitCode::from(case.exit),
                "{} {format}: {}",
                case.name,
                String::from_utf8_lossy(&stderr)
            );
            assert_eq!(
                String::from_utf8(stdout).unwrap(),
                String::from_utf8(fixture(&format!("{dir}/{}.{format}.golden", case.name)))
                    .unwrap(),
                "{} {format}",
                case.name
            );
            if case.exit == 0 {
                assert!(stderr.is_empty());
            } else {
                assert_eq!(
                    String::from_utf8(stderr).unwrap(),
                    String::from_utf8(fixture(&format!("{dir}/{}.stderr.golden", case.name)))
                        .unwrap(),
                    "{}",
                    case.name
                );
            }
            let heads = server.heads();
            assert_eq!(heads.len(), case.replies.len(), "{}", case.name);
            for ((head, body), reply) in heads.iter().zip(server.bodies()).zip(&case.replies) {
                let product = if reply.route.starts_with("/accounts-api/") {
                    "dashboard"
                } else {
                    "iam"
                };
                assert!(head.starts_with(
                    format!("{} /{product}{} ", reply.method, reply.route).as_bytes()
                ));
                if matches!(
                    case.name.as_str(),
                    "attach-service-account-policy" | "detach-service-account-policy"
                ) && matches!(reply.method.as_str(), "POST" | "DELETE")
                {
                    assert!(body.is_empty());
                }
                if let Some(expected) = &reply.request {
                    assert!(serde_json::from_slice::<Value>(&body).unwrap() == *expected);
                }
            }
        }
    }
}

#[tokio::test]
async fn group_write_guards_refuse_before_requests() {
    let server = Server::start(vec![]);
    for (args, expected) in [
        (
            &["iam", "delete-group", "--group-id", "group-1"][..],
            "InvalidUsage",
        ),
        (
            &[
                "--read-only",
                "iam",
                "delete-group",
                "--group-id",
                "group-1",
                "--yes",
            ][..],
            "ReadOnly",
        ),
        (
            &["--read-only", "iam", "create-group", "--name", "team"][..],
            "ReadOnly",
        ),
        (
            &[
                "--read-only",
                "iam",
                "update-group",
                "--group-id",
                "group-1",
            ][..],
            "ReadOnly",
        ),
    ] {
        let (exit, stdout, stderr) = invoke(args, &server, BEARER).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains(expected));
    }
    assert_eq!(server.requests(), 0);
}

#[tokio::test]
async fn policy_document_file_overrides_json_after_local_guards() {
    let dir = std::env::temp_dir().join(format!("greennode-policy-doc-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    struct RemoveDir(std::path::PathBuf);
    impl Drop for RemoveDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = RemoveDir(dir);
    let path = dir.0.join("policy.json");
    std::fs::write(&path, br#"{"Statements":[{"Effect":"allow","Actions":["iam:ListActions"],"Resources":["*"],"Condition":{"equals":{"\uD800":"\uD800"}}}]}"#).unwrap();
    let server = Server::start(vec![
        response(200, br#"{"userId":"usr-1","userType":"iam-user"}"#.to_vec()),
        response(
            200,
            br#"[{"action":"CreatePolicy","label":"Write"}]"#.to_vec(),
        ),
        response(201, br#"{"id":"pol-1"}"#.to_vec()),
        response(200, br#"{"id":"pol-1"}"#.to_vec()),
    ]);
    let path = path.to_str().unwrap();
    let args = [
        "iam",
        "create-policy",
        "--name",
        "reader",
        "--document-file",
        path,
        "--cli-input-json",
        r#"{"Statements":[{"Effect":"allow","Actions":["iam:*"],"Resources":["*"]}]}"#,
    ];
    let (exit, _, stderr) = invoke(&args, &server, BEARER).await;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&stderr)
    );
    let bod = server.bodies();
    assert!(
        serde_json::from_slice::<Value>(&bod[2]).unwrap()
            == serde_json::json!({
                "name":"reader", "description":"", "statements":[{"effect":"allow", "actions":["iam:ListActions"], "resources":["*"], "condition":{"equals":{"�":"�"}}}]
            })
    );

    let server = Server::start(vec![]);
    let invalid = async |diagnostic: &str| {
        let (exit, stdout, stderr) = invoke(&args[..6], &server, BEARER).await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains(diagnostic));
        assert_eq!(server.requests(), 0);
    };
    for malformed in [
        r#"{"Statement":[]}"#,
        r#"{"statements":[],"Statements":[]}"#,
        r#"{"statements":[]}{}"#,
    ] {
        std::fs::write(path, malformed).unwrap();
        invalid("invalid policy document").await;
    }
    std::fs::write(path, vec![b' '; (64 << 10) + 1]).unwrap();
    invalid("file exceeds 64 KiB").await;
    std::fs::remove_file(path).unwrap();
    #[cfg(unix)]
    {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        invalid("file must be regular").await;
        let (exit, _, stderr) = invoke(
            &[
                "--read-only",
                "iam",
                "create-policy",
                "--name",
                "reader",
                "--document-file",
                path,
            ],
            &server,
            BEARER,
        )
        .await;
        assert_eq!(exit, ExitCode::from(2));
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    }
}

#[tokio::test]
async fn policy_create_uses_last_top_level_json_values() {
    let server = Server::start(vec![
        response(200, br#"{"userId":"usr-1","userType":"iam-user"}"#.to_vec()),
        response(
            200,
            br#"[{"action":"CreatePolicy","label":"Write"}]"#.to_vec(),
        ),
        response(201, br#"{"id":"pol-1"}"#.to_vec()),
        response(200, br#"{"id":"pol-1"}"#.to_vec()),
    ]);
    let (exit, _, stderr) = invoke(&["iam", "create-policy", "--cli-input-json", r#"{"Name":3,"Name":"reader","Statements":false,"Statements":[{"Effect":"allow","Actions":["iam:ListActions"],"Resources":["*"]}]}"#], &server, BEARER).await;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&stderr)
    );
    assert_eq!(server.requests(), 4);
    let body: Value = serde_json::from_slice(&server.bodies()[2]).unwrap();
    assert!(body["name"] == "reader");
}

#[tokio::test]
async fn truncated_policy_create_rejection_is_uncertain() {
    let server = Server::start(vec![
        response(200, br#"{"userId":"usr-1","userType":"iam-user"}"#.to_vec()),
        response(
            200,
            br#"[{"action":"CreatePolicy","label":"Write"}]"#.to_vec(),
        ),
        Response::Http {
            status: "404 Not Found".into(),
            headers: vec![("Content-Length".into(), "20".into())],
            body: b"{}".to_vec(),
        },
    ]);
    let args = [
        "iam",
        "create-policy",
        "--cli-input-json",
        r#"{"Name":"reader","Statements":[{"Effect":"allow","Actions":["iam:ListActions"],"Resources":["*"]}]}"#,
    ];
    let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
    assert_eq!(exit, ExitCode::from(1));
    assert!(stdout.is_empty());
    let error: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "RequestFailed");
    assert!(error["error"]["status"].is_null());
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("check with list-policies --name before creating it again")
    );
    assert_eq!(server.requests(), 3);

    let server = Server::start(vec![
        response(200, br#"{"userId":"usr-1","userType":"iam-user"}"#.to_vec()),
        response(
            200,
            br#"[{"action":"CreatePolicy","label":"Write"}]"#.to_vec(),
        ),
        response(404, b"{}".to_vec()),
    ]);
    let (exit, stdout, stderr) = invoke(&args, &server, BEARER).await;
    assert_eq!(exit, ExitCode::from(4));
    assert!(stdout.is_empty());
    let error: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "NotFound");
    assert_eq!(error["error"]["status"], 404);
    assert_eq!(error["error"]["message"], "Not Found");
    assert_eq!(server.requests(), 3);
}
