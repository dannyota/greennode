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
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

#[tokio::test]
async fn writes_match_go_routes_exit_codes_and_output() {
    check_go_goldens(|case| {
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
    check_go_goldens(|case| case == "create-group-redirect").await;
}

#[tokio::test]
async fn create_group_malformed_confirmation_matches_go() {
    check_go_goldens(|case| case == "create-group-malformed-confirmation").await;
}

#[tokio::test]
async fn create_group_path_invalid_id_recovery_matches_go() {
    check_go_goldens(|case| case == "create-group-invalid-path-id").await;
}

async fn check_go_goldens(select: impl Fn(&str) -> bool) {
    let cases: Vec<Case> = serde_json::from_slice(&fixture("writes/cases.json")).unwrap();
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
                String::from_utf8(fixture(&format!("writes/{}.{format}.golden", case.name)))
                    .unwrap(),
                "{} {format}",
                case.name
            );
            if case.exit == 0 {
                assert!(stderr.is_empty());
            } else {
                assert_eq!(
                    String::from_utf8(stderr).unwrap(),
                    String::from_utf8(fixture(&format!("writes/{}.stderr.golden", case.name)))
                        .unwrap(),
                    "{}",
                    case.name
                );
            }
            let heads = server.heads();
            assert_eq!(heads.len(), case.replies.len(), "{}", case.name);
            for (head, reply) in heads.iter().zip(&case.replies) {
                let product = if reply.route.starts_with("/accounts-api/") {
                    "dashboard"
                } else {
                    "iam"
                };
                assert!(head.starts_with(
                    format!("{} /{product}{} ", reply.method, reply.route).as_bytes()
                ));
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
