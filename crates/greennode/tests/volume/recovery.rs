use super::*;

#[test]
fn create_volume_stderr_screens_the_operation_and_keeps_reconciliation_advice() {
    for bearer in ["CreateVolume", "before"] {
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            ),
            response("200 OK", QUOTE),
            response(
                "500 Internal Server Error",
                r#"{"message":"provider refused"}"#,
            ),
        ]);
        let (exit, stdout, stderr) = invoke_with_bearer(
            &[
                "create-volume",
                "--name",
                "toy-volume",
                "--zone-id",
                "zone-1",
                "--volume-type-id",
                "type-1",
                "--size",
                "1",
                "--max-price",
                "3200",
                "--no-wait",
            ],
            &server,
            false,
            bearer,
        );
        assert_eq!(exit, ExitCode::FAILURE);
        assert!(stdout.is_empty());
        assert!(!std::str::from_utf8(&stderr).unwrap().contains(bearer));
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        let error = &envelope["error"];
        assert_eq!(error["code"], "ServerError");
        assert_eq!(error["status"], 500);
        let message = error["message"].as_str().unwrap();
        if bearer == "before" {
            assert_eq!(message, "provider refused");
        } else {
            assert!(message.contains("Check for an existing volume"));
        }
        assert_eq!(server.requests(), 3);
    }
}

#[tokio::test]
async fn create_volume_cli_keeps_not_settled_when_recovery_text_is_withheld() {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Rotating(AtomicUsize, &'static str);
    impl greennode_sdk::CredentialsProvider for Rotating {
        fn token<'a>(
            &'a self,
            _: &'a RequestContext,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<greennode_sdk::Token, greennode_sdk::Error>> + Send + 'a,
            >,
        > {
            let index = self.0.fetch_add(1, Ordering::Relaxed);
            let token = ["toy-inventory", "toy-quote", "Audit", self.1]
                .get(index)
                .copied()
                .unwrap_or("toy-later");
            Box::pin(async move { Ok(greennode_sdk::Token::new(Secret::new(token), None)) })
        }
        fn invalidate<'a>(
            &'a self,
            _: &'a RequestContext,
            _: &'a Secret,
        ) -> Pin<Box<dyn Future<Output = Result<(), greennode_sdk::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    for confirmation in ["toy-confirm", "Check"] {
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            ),
            response("200 OK", QUOTE),
            response("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            response("403 Forbidden", "{}"),
        ]);
        let options = ConfigOptions::default()
            .with_credentials_provider(Arc::new(Rotating(AtomicUsize::new(0), confirmation)))
            .with_endpoint_overrides(EndpointOverrides {
                vserver: Some(format!("{}/", server.origin)),
                portal: Some(format!("{}/", server.origin)),
                ..Default::default()
            });
        let options = with_billing_loopback(options, &server);
        let (context, _) = RequestContext::new();
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let status = run_invocation(Invocation {
            args: [
                "greennode",
                "--project-id",
                "proj-1",
                "volume",
                "create-volume",
                "--name",
                "toy-volume",
                "--zone-id",
                "zone-1",
                "--volume-type-id",
                "type-1",
                "--size",
                "1",
                "--max-price",
                "3200",
            ]
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
        })
        .await;
        assert_eq!(status, ExitCode::FAILURE);
        assert_eq!(server.requests(), 4);
        let envelope: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "NotSettled");
        let expected = if confirmation == "Check" {
            "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: volume.GetVolume: Forbidden (status 403)"
        } else {
            "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: volume.GetVolume: Forbidden (status 403)\nCheck for an existing volume with the exact name before retrying."
        };
        assert_eq!(envelope["error"]["message"], expected);
    }
}

#[test]
fn create_gateway_failure_preserves_http_error_and_recovery_guidance() {
    assert_create_outcome(
        vec![response("502 Bad Gateway", "")],
        serde_json::json!({"error":{"code":"ServerError", "status":502, "operation":"volume.CreateVolume", "message":"Bad Gateway\nCheck for an existing volume with the exact name before retrying."}}),
    );
}

#[test]
fn create_missing_id_retains_request_failure_metadata() {
    assert_create_outcome(
        vec![response("202 Accepted", r#"{"data":{}}"#)],
        serde_json::json!({"error":{"code":"RequestFailed", "status":202, "operation":"volume.CreateVolume", "message":"create response had no id\nCheck for an existing volume with the exact name before retrying."}}),
    );
}

#[test]
fn confirmed_create_failure_keeps_reason_and_guidance() {
    assert_create_outcome(
        vec![
            response("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            response(
                "200 OK",
                r#"{"data":{"uuid":"volume-1","name":"toy-volume","status":"ERROR","size":1,"volumeTypeId":"type-1"}}"#,
            ),
        ],
        serde_json::json!({"error":{"code":"WriteFailed", "message":"vngcloud: write failed: volume.CreateVolume: volume volume-1 is ERROR\nCheck for an existing volume with the exact name before retrying."}}),
    );
}

#[test]
fn malformed_volume_acknowledgements_return_statusless_decode_errors() {
    for body in ["{broken", r#"{"data":{"uuid":7}}"#] {
        assert_create_outcome(
            vec![response("202 Accepted", body)],
            serde_json::json!({"error":{"code":"RequestFailed", "operation":"volume.CreateVolume", "message":"volume.CreateVolume: request failed: response failed to decode\nCheck for an existing volume with the exact name before retrying."}}),
        );
    }
}

#[test]
fn missing_volume_id_keeps_accepted_status_when_operation_is_screened() {
    let server = Server::start(vec![
        response(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
        ),
        response("200 OK", QUOTE),
        response("202 Accepted", r#"{"data":{}}"#),
    ]);
    let (exit, stdout, stderr) = invoke_with_bearer(
        &[
            "create-volume",
            "--name",
            "toy-volume",
            "--zone-id",
            "zone-1",
            "--volume-type-id",
            "type-1",
            "--size",
            "1",
            "--max-price",
            "3200",
        ],
        &server,
        false,
        "CreateVolume",
    );
    assert_eq!(exit, ExitCode::FAILURE);
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "RequestFailed");
    assert_eq!(error["error"]["status"], 202);
    assert!(error["error"].get("operation").is_none());
}

#[tokio::test]
async fn volume_confirmation_sleep_reports_the_nested_context_reason() {
    use greennode_sdk::testing::{ResponseBodyClock, with_clock};
    use std::{sync::Arc, time::Duration};
    for (deadline, reason) in [
        (false, "context canceled"),
        (true, "context deadline exceeded"),
    ] {
        let server = Server::start(vec![
            response(
                "200 OK",
                r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
            ),
            response("200 OK", QUOTE),
            response("202 Accepted", r#"{"data":{"uuid":"volume-1"}}"#),
            response(
                "200 OK",
                r#"{"data":{"uuid":"volume-1","name":"toy-volume","status":"CREATING","size":1,"volumeTypeId":"type-1"}}"#,
            ),
        ]);
        let clock = Arc::new(ResponseBodyClock::new());
        let options = with_clock(
            ConfigOptions::default()
                .with_static_token(Secret::new(BEARER))
                .with_endpoint_overrides(EndpointOverrides {
                    vserver: Some(format!("{}/", server.origin)),
                    portal: Some(format!("{}/", server.origin)),
                    ..Default::default()
                }),
            clock.clone(),
        );
        let options = with_billing_loopback(options, &server);
        let (context, cancel) = RequestContext::new();
        let bound = Duration::from_secs(5);
        let context = if deadline {
            context.child_with_deadline(greennode_sdk::Clock::monotonic_now(&*clock) + bound)
        } else {
            context
        };
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let work = run_invocation(Invocation {
            args: [
                "greennode",
                "--project-id",
                "proj-1",
                "volume",
                "create-volume",
                "--name",
                "toy-volume",
                "--zone-id",
                "zone-1",
                "--volume-type-id",
                "type-1",
                "--size",
                "1",
                "--max-price",
                "3200",
            ]
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
            clock: clock.as_ref(),
            context: &context,
        });
        let interrupt = async {
            clock.wait_for_sleep(Duration::from_secs(2)).await;
            if deadline {
                clock.advance(bound);
            } else {
                cancel.cancel();
            }
        };
        let (exit, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(work, interrupt)
        })
        .await
        .unwrap();
        assert_eq!(exit, ExitCode::FAILURE);
        assert_eq!(server.requests(), 4);
        let error: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
        assert_eq!(error["error"]["code"], "NotSettled");
        assert_eq!(
            error["error"]["message"],
            format!(
                "vngcloud: write outcome not confirmed: volume.CreateVolume: volume volume-1: {reason}\nCheck for an existing volume with the exact name before retrying."
            )
        );
    }
}

fn assert_create_outcome(replies: Vec<Response>, expected: serde_json::Value) {
    let mut responses = vec![
        response(
            "200 OK",
            r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
        ),
        response("200 OK", QUOTE),
    ];
    responses.extend(replies);
    let count = responses.len();
    let server = Server::start(responses);
    let (exit, stdout, stderr) = invoke_mode(
        &[
            "create-volume",
            "--name",
            "toy-volume",
            "--zone-id",
            "zone-1",
            "--volume-type-id",
            "type-1",
            "--size",
            "1",
            "--max-price",
            "3200",
        ],
        &server,
        false,
    );
    assert_eq!(exit, ExitCode::FAILURE);
    assert_eq!(server.requests(), count);
    if expected["error"]["code"] == "WriteFailed" {
        let partial: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(partial["Volume"]["Status"], "ERROR");
    } else {
        assert!(stdout.is_empty());
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stderr).unwrap(),
        expected
    );
}
