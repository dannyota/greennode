use super::*;

const ID: &[&str] = &["--cdn-id", "cdn-1"];
const UPDATE: &[&str] = &[
    "--cdn-id",
    "cdn-1",
    "--origin-host-header",
    "new.example.test",
    "--no-wait",
];
const ENABLE: &[&str] = &["--cdn-id", "cdn-1", "--no-wait"];
const CONFIRMED: &[&str] = &["--cdn-id", "cdn-1", "--yes"];
const PURGE: &[&str] = &[
    "--cdn-domain",
    "cdn.example.test",
    "--cli-input-json",
    r#"{"Paths":["/index.html"]}"#,
];

macro_rules! wire_fixture {
    ($file:literal) => {
        include_str!(concat!("../../../greennode-sdk/tests/fixtures/cdn/", $file))
    };
}
const ACTIVE: &str = wire_fixture!("webaccelerator-detail.json");
const DISABLED: &str = wire_fixture!("webaccelerator-detail-disabled.json");
const DEPLOYING: &str = wire_fixture!("webaccelerator-detail-deploying.json");
const TOGGLED: &str = wire_fixture!("write-toggle.json");

macro_rules! case {
    ($operation:literal, $flags:expr, $responses:expr) => {
        (
            $operation,
            $flags,
            $responses,
            [
                include_str!(concat!("../fixtures/cdn/", $operation, ".json.golden")),
                include_str!(concat!("../fixtures/cdn/", $operation, ".table.golden")),
                include_str!(concat!("../fixtures/cdn/", $operation, ".text.golden")),
            ],
        )
    };
}

fn reply(body: &str) -> Response {
    response("200 OK", body, "application/json")
}

#[test]
fn all_cdn_writes_match_the_go_renderer() {
    let cases = [
        case!(
            "update-web-accelerator",
            UPDATE,
            &[ACTIVE, wire_fixture!("write-update.json"), DEPLOYING][..]
        ),
        case!(
            "delete-web-accelerator",
            CONFIRMED,
            &[DISABLED, TOGGLED][..]
        ),
        case!(
            "enable-web-accelerator",
            ENABLE,
            &[DISABLED, TOGGLED, DEPLOYING][..]
        ),
        case!(
            "disable-web-accelerator",
            CONFIRMED,
            &[ACTIVE, TOGGLED, DISABLED][..]
        ),
        case!(
            "purge-paths",
            PURGE,
            &[wire_fixture!("purge-success.json")][..]
        ),
    ];
    for (operation, flags, responses, goldens) in cases {
        for (format, golden) in ["json", "table", "text"].into_iter().zip(goldens) {
            let server = Server::start(responses.iter().map(|body| reply(body)).collect());
            let mut args = vec!["--output", format, "cdn", operation];
            args.extend_from_slice(flags);
            let (status, stdout, stderr) = invoke(&args, &server, false, Some(KEY), true);
            assert_eq!(status, ExitCode::SUCCESS, "{operation} {format}");
            assert!(stderr.is_empty(), "{operation} {format}");
            assert_eq!(
                String::from_utf8(stdout).unwrap(),
                golden,
                "{operation} {format}"
            );
            assert_eq!(server.requests(), responses.len());
        }
    }
}

#[test]
fn local_confirmation_and_read_only_guards_send_nothing() {
    for (operation, flags) in [
        ("delete-web-accelerator", ID),
        ("disable-web-accelerator", ID),
    ] {
        let server = Server::start(vec![]);
        let mut args = vec!["cdn", operation];
        args.extend_from_slice(flags);
        let (status, stdout, stderr) = invoke(&args, &server, false, Some(KEY), true);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "InvalidUsage");
        assert_eq!(server.requests(), 0);
    }
    for (operation, flags) in [
        ("update-web-accelerator", UPDATE),
        ("delete-web-accelerator", CONFIRMED),
        ("enable-web-accelerator", ENABLE),
        ("disable-web-accelerator", CONFIRMED),
        ("purge-paths", PURGE),
    ] {
        let server = Server::start(vec![]);
        let mut args = vec!["--read-only", "cdn", operation];
        args.extend_from_slice(flags);
        let (status, stdout, stderr) = invoke(&args, &server, false, Some(KEY), true);
        assert_eq!(status, ExitCode::from(2));
        assert!(stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "ReadOnly");
        assert_eq!(server.requests(), 0);
    }
}

#[test]
fn write_errors_use_cdn_identities_and_settlement_partials_ignore_queries() {
    let unknown = r#"{"success":true,"data":{"cdnId":"cdn-1","status":42}}"#;
    let cooldown =
        r#"{"success":false,"code":202,"message":"Last CDN flush cache time is too recent"}"#;
    for (operation, flags, replies, identity, partial) in [
        (
            "delete-web-accelerator",
            CONFIRMED,
            vec![DEPLOYING],
            "ResourceBusy",
            false,
        ),
        (
            "disable-web-accelerator",
            CONFIRMED,
            vec![unknown],
            "UnexpectedStatus",
            false,
        ),
        ("purge-paths", PURGE, vec![cooldown], "PurgeCooldown", false),
        (
            "update-web-accelerator",
            UPDATE,
            vec![ACTIVE, TOGGLED, "{}"],
            "NotSettled",
            true,
        ),
    ] {
        let server = Server::start(replies.iter().map(|body| reply(body)).collect());
        let mut args = vec!["--query", "WebAccelerator.CDNID", "cdn", operation];
        args.extend_from_slice(flags);
        let (status, stdout, stderr) = invoke(&args, &server, false, Some(KEY), true);
        assert_eq!(status, ExitCode::from(1));
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], identity);
        assert!(envelope["error"]["status"].is_null() && envelope["error"]["operation"].is_null());
        if partial {
            let result: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
            assert_eq!(result["WebAccelerator"]["CDNID"], "cdn-1");
        } else {
            assert!(stdout.is_empty());
        }
        assert_eq!(server.requests(), replies.len());
    }
}
