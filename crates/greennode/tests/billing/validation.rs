use super::*;

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    args: Vec<String>,
    http_status: u16,
    body: String,
    exit: u8,
    code: Option<String>,
    status: Option<u16>,
    message: Option<String>,
    bearer: Option<String>,
    #[serde(default)]
    stdout: String,
}

struct ValidationSource;

impl ConfigSource for ValidationSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        match name.to_str()? {
            "GREENNODE_REGION" => Some("hcm-3".into()),
            "VNGCLOUD_READ_ONLY" => Some("false".into()),
            _ => None,
        }
    }
}

#[test]
fn billing_response_validation_matches_go_v1_8_0() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../fixtures/billing/validation-go-v1.8.0.json"
    ))
    .unwrap();
    let mut mismatches = Vec::new();
    for case in cases {
        let server = Server::start(vec![response(
            &format!("{} Fixture", case.http_status),
            case.body,
        )]);
        let mut args = vec!["--output", "json", "billing"];
        args.extend(case.args.iter().map(String::as_str));
        let (exit, stdout, stderr) = invoke_with_source(
            &args,
            Some(&server),
            case.bearer.as_deref().unwrap_or("test-token"),
            &ValidationSource,
        );
        assert_eq!(server.requests(), 1, "{}", case.name);
        let error = serde_json::from_slice::<serde_json::Value>(&stderr).ok();
        let detail = error.as_ref().map(|error| &error["error"]);
        let code = detail.and_then(|error| error["code"].as_str());
        let status = detail.and_then(|error| error["status"].as_u64());
        let message = detail.and_then(|error| error["message"].as_str());
        let differences = [
            (exit != ExitCode::from(case.exit), "exit"),
            (stdout != case.stdout.as_bytes(), "stdout"),
            (code != case.code.as_deref(), "code"),
            (status != case.status.map(u64::from), "status"),
            (
                case.message
                    .as_deref()
                    .is_some_and(|expected| message != Some(expected)),
                "message",
            ),
        ]
        .into_iter()
        .filter_map(|(differs, field)| differs.then_some(field))
        .collect::<Vec<_>>();
        if !differences.is_empty() {
            mismatches.push((case.name, differences));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}
