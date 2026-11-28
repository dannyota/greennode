use super::*;
use greennode_sdk::testing::{Product, ResponseBodyClock, with_clock, with_write_loopback};
use greennode_sdk::volume::{
    AttachVolumeInput, Client, CreateVolumeInput, DeleteVolumeInput, DetachVolumeInput,
    ResizeVolumeInput,
};
use greennode_sdk::{
    CancelHandle, Clock, Config, Error, ErrorClass, Failure, RecoveryAction, WriteDispatch,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Write {
    Attach,
    Detach,
    Delete,
    Resize,
    Create,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Settled,
    Failed,
    Timeout,
    CallerDeadline,
    CancelSleep,
    CancelRead,
    Refused,
    CancelAfterFailed,
    CancelAfterSettled,
    TimeoutAfterFailed,
    LongId,
    Collision,
}

impl Write {
    fn command(self) -> &'static str {
        match self {
            Self::Attach => "attach-volume",
            Self::Detach => "detach-volume",
            Self::Delete => "delete-volume",
            Self::Resize => "resize-volume",
            Self::Create => "create-volume",
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::Attach => "volume.AttachVolume",
            Self::Detach => "volume.DetachVolume",
            Self::Delete => "volume.DeleteVolume",
            Self::Resize => "volume.ResizeVolume",
            Self::Create => "volume.CreateVolume",
        }
    }

    fn observed(self) -> (&'static str, u64, bool) {
        match self {
            Self::Attach => ("IN-USE", 1, true),
            Self::Detach | Self::Create => ("AVAILABLE", 1, false),
            Self::Delete => ("DELETED", 1, false),
            Self::Resize => ("AVAILABLE", 2, false),
        }
    }

    fn timeout_reason(self) -> &'static str {
        match self {
            Self::Attach => {
                "did not confirm attach to server server-1 within 5m0s; run this operation again to check"
            }
            Self::Detach => "did not confirm detach within 5m0s; run this operation again to check",
            Self::Delete => {
                "did not reach 404 or DELETED within 5m0s; delete was sent and a rerun is safe"
            }
            Self::Resize => {
                "did not confirm resize to 2 GB within 5m0s; call GetVolume to check its status"
            }
            Self::Create => "did not reach AVAILABLE within 5m0s",
        }
    }

    fn replies(self, id: &str, outcome: Outcome) -> Vec<Response> {
        let original = if self == Self::Detach {
            "IN-USE"
        } else {
            "AVAILABLE"
        };
        let initial = || volume(id, original, 1, self == Self::Detach);
        let mut replies = match self {
            Self::Create => vec![
                response(
                    "200 OK",
                    r#"{"listData":[],"page":1,"pageSize":10000,"totalPage":0,"totalItem":0}"#,
                ),
                response("200 OK", QUOTE),
            ],
            Self::Resize => vec![initial(), response("200 OK", QUOTE)],
            Self::Detach => vec![
                initial(),
                response(
                    "200 OK",
                    r#"{"data":{"status":"STOPPED","bootVolumeId":"boot-1"}}"#,
                ),
            ],
            Self::Attach | Self::Delete => vec![initial()],
        };
        let ack = if self == Self::Create {
            json!({"data":{"uuid":id}}).to_string()
        } else {
            String::new()
        };
        replies.push(response("202 Accepted", &ack));
        match outcome {
            Outcome::Accepted => {}
            Outcome::Settled if self == Self::Delete => {
                replies.push(response("404 Not Found", "{}"))
            }
            Outcome::Settled | Outcome::CancelAfterSettled => {
                let (status, size, attached) = self.observed();
                replies.push(volume(id, status, size, attached));
            }
            Outcome::Failed | Outcome::CancelAfterFailed | Outcome::TimeoutAfterFailed => {
                replies.push(volume(id, "ERROR", 1, false));
            }
            Outcome::Refused => replies.push(response(
                "403 Forbidden",
                r#"{"message":"confirmation refused by provider"}"#,
            )),
            Outcome::CancelRead => {
                let (head, _) = tokio::sync::oneshot::channel();
                replies.push(Response::Stall(head, 10));
            }
            _ => replies.push(volume(id, "UPDATING", 1, false)),
        }
        replies
    }

    fn args(self, id: &str, no_wait: bool) -> Vec<OsString> {
        let mut args = vec![
            "greennode",
            "--project-id",
            "proj-1",
            "volume",
            self.command(),
        ];
        match self {
            Self::Create => args.extend([
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
            ]),
            Self::Resize => args.extend([
                "--yes",
                "--volume-id",
                id,
                "--size",
                "2",
                "--max-price",
                "3200",
            ]),
            Self::Delete => args.extend(["--yes", "--volume-id", id]),
            Self::Detach => args.extend(["--yes", "--volume-id", id, "--server-id", "server-1"]),
            Self::Attach => args.extend(["--volume-id", id, "--server-id", "server-1"]),
        }
        args.extend(no_wait.then_some("--no-wait"));
        args.into_iter().map(OsString::from).collect()
    }

    async fn sdk(
        self,
        client: &Client,
        ctx: &RequestContext,
        id: &str,
        no_wait: bool,
    ) -> (Option<Error>, Value) {
        match self {
            Self::Create => project(
                client
                    .create_volume(
                        ctx,
                        CreateVolumeInput {
                            name: "toy-volume".into(),
                            size: 1,
                            zone_id: "zone-1".into(),
                            volume_type_id: "type-1".into(),
                            max_price: 3200.0,
                            no_wait,
                            ..Default::default()
                        },
                    )
                    .await,
            ),
            Self::Resize => project(
                client
                    .resize_volume(
                        ctx,
                        ResizeVolumeInput {
                            volume_id: id.into(),
                            size: 2,
                            max_price: 3200.0,
                            no_wait,
                        },
                    )
                    .await,
            ),
            Self::Delete => project(
                client
                    .delete_volume(
                        ctx,
                        DeleteVolumeInput {
                            volume_id: id.into(),
                            no_wait,
                        },
                    )
                    .await,
            ),
            Self::Detach => project(
                client
                    .detach_volume(
                        ctx,
                        DetachVolumeInput {
                            volume_id: id.into(),
                            server_id: "server-1".into(),
                            no_wait,
                            allow_running: false,
                        },
                    )
                    .await,
            ),
            Self::Attach => project(
                client
                    .attach_volume(
                        ctx,
                        AttachVolumeInput {
                            volume_id: id.into(),
                            server_id: "server-1".into(),
                            no_wait,
                        },
                    )
                    .await,
            ),
        }
    }
}

fn project<T: serde::Serialize>(result: Result<T, Failure<T>>) -> (Option<Error>, Value) {
    match result {
        Ok(output) => (None, serde_json::to_value(output).unwrap()),
        Err(failure) => {
            let (error, partial, dispatch) = failure.into_parts();
            assert_eq!(dispatch, WriteDispatch::MayHaveBeenSent);
            (Some(error), serde_json::to_value(partial.unwrap()).unwrap())
        }
    }
}

fn volume(id: &str, status: &str, size: u64, attached: bool) -> Response {
    let server = attached.then_some("server-1");
    response(
        "200 OK",
        &json!({"data": {
            "uuid":id, "status":status, "name":"toy-volume", "size":size, "volumeTypeId":"type-1",
            "serverId":server, "serverIdList":server.into_iter().collect::<Vec<_>>()
        }})
        .to_string(),
    )
}

struct TestWait {
    clock: Arc<ResponseBodyClock>,
    cancel: CancelHandle,
    outcome: Outcome,
}

impl TestWait {
    async fn interrupt(&self, confirmations_at: usize) {
        match self.outcome {
            Outcome::CancelRead => {
                self.clock.wait_for_body(confirmations_at).await;
                self.cancel.cancel();
            }
            Outcome::Timeout
            | Outcome::CallerDeadline
            | Outcome::CancelSleep
            | Outcome::LongId
            | Outcome::Collision => {
                self.clock.wait_for_sleep(Duration::from_secs(2)).await;
                match self.outcome {
                    Outcome::CancelSleep => self.cancel.cancel(),
                    Outcome::CallerDeadline => self.clock.advance(Duration::from_secs(5)),
                    _ => self.clock.advance(Duration::from_secs(300)),
                }
            }
            _ => {}
        }
    }
}

struct Call {
    server: Server,
    wait: TestWait,
    context: RequestContext,
    options: ConfigOptions,
    requests: usize,
}

impl Call {
    fn new(write: Write, outcome: Outcome, id: &str, bearer: &str) -> Self {
        let replies = write.replies(id, outcome);
        let requests = replies.len();
        let server = Server::start(replies);
        let (context, cancel) = RequestContext::new();
        let read_cancel = cancel.clone();
        let clock = Arc::new(
            ResponseBodyClock::new().on_volume_confirmation(move |clock| match outcome {
                Outcome::CancelAfterFailed | Outcome::CancelAfterSettled => read_cancel.cancel(),
                Outcome::TimeoutAfterFailed => clock.advance(Duration::from_secs(300)),
                _ => {}
            }),
        );
        let context = if outcome == Outcome::CallerDeadline {
            context.child_with_deadline(clock.monotonic_now() + Duration::from_secs(5))
        } else {
            context
        };
        let options = with_clock(
            ConfigOptions::default()
                .with_static_token(Secret::new(bearer))
                .with_endpoint_overrides(EndpointOverrides {
                    vserver: Some(format!("{}/", server.origin)),
                    portal: Some(format!("{}/", server.origin)),
                    ..Default::default()
                }),
            clock.clone(),
        );
        let options = with_write_loopback(
            Product::Vserver,
            with_billing_loopback(options, &server),
            &server,
        );
        Self {
            server,
            wait: TestWait {
                clock,
                cancel,
                outcome,
            },
            context,
            options,
            requests,
        }
    }

    async fn sdk(self, write: Write, id: &str) -> (Option<Error>, Value) {
        let client = Client::new(
            &Config::new(
                self.options
                    .with_project_id("proj-1".into())
                    .with_region("hcm-3".into()),
            )
            .unwrap(),
        );
        let work = write.sdk(
            &client,
            &self.context,
            id,
            self.wait.outcome == Outcome::Accepted,
        );
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(work, self.wait.interrupt(self.requests))
        })
        .await
        .unwrap();
        assert_eq!(self.server.requests(), self.requests);
        result
    }

    async fn cli(self, write: Write, id: &str) -> (ExitCode, Value, Vec<u8>) {
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let work = run_invocation(Invocation {
            args: write.args(id, self.wait.outcome == Outcome::Accepted),
            source: &Source,
            config_options: self.options,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: self.wait.clock.as_ref(),
            context: &self.context,
        });
        let (exit, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(work, self.wait.interrupt(self.requests))
        })
        .await
        .unwrap();
        assert_eq!(self.server.requests(), self.requests);
        (exit, serde_json::from_slice(&stdout).unwrap(), stderr)
    }
}

async fn exercise(outcome: Outcome) {
    let long = "v".repeat(512);
    let id = if outcome == Outcome::LongId {
        &long
    } else {
        "volume-1"
    };
    let bearer = if outcome == Outcome::Collision {
        "repeated"
    } else {
        BEARER
    };
    for write in [
        Write::Attach,
        Write::Detach,
        Write::Delete,
        Write::Resize,
        Write::Create,
    ] {
        let label = format!("{write:?}/{outcome:?}");
        let (sdk_error, partial) = Call::new(write, outcome, id, bearer).sdk(write, id).await;
        let (exit, cli_output, stderr) = Call::new(write, outcome, id, bearer).cli(write, id).await;
        assert!(
            !String::from_utf8_lossy(&stderr).contains(bearer),
            "{label}"
        );
        assert!(!cli_output.to_string().contains(bearer), "{label}");
        if write == Write::Delete {
            assert_eq!(partial, json!({}), "{label}");
            assert_eq!(cli_output, json!({}), "{label}");
        } else {
            let status = match outcome {
                Outcome::Settled | Outcome::CancelAfterSettled => write.observed().0,
                Outcome::Failed | Outcome::CancelAfterFailed | Outcome::TimeoutAfterFailed => {
                    "ERROR"
                }
                Outcome::Accepted | Outcome::CancelRead | Outcome::Refused => match write {
                    Write::Create => "",
                    Write::Detach => "IN-USE",
                    _ => "AVAILABLE",
                },
                _ => "UPDATING",
            };
            let size = match outcome {
                Outcome::Settled | Outcome::CancelAfterSettled => write.observed().1,
                Outcome::Accepted | Outcome::CancelRead | Outcome::Refused
                    if write == Write::Create =>
                {
                    0
                }
                _ => 1,
            };
            for output in [&partial, &cli_output] {
                assert_eq!(output["Volume"]["UUID"], id, "{label}");
                assert_eq!(output["Volume"]["Status"], status, "{label}");
                assert_eq!(output["Volume"]["Size"], size, "{label}");
                if matches!(write, Write::Attach | Write::Detach) {
                    assert_eq!(output["Changed"], true, "{label}");
                }
            }
        }
        if matches!(outcome, Outcome::Accepted | Outcome::Settled) {
            assert!(sdk_error.is_none(), "{label}");
            assert_eq!(exit, ExitCode::SUCCESS, "{label}");
            assert!(stderr.is_empty(), "{label}");
            continue;
        }
        assert_eq!(exit, ExitCode::FAILURE, "{label}");
        let envelope: Value = serde_json::from_slice(&stderr).unwrap();
        let message = envelope["error"]["message"].as_str().unwrap();
        let error = sdk_error.unwrap();
        assert_eq!(
            error.api_error().is_some(),
            matches!(outcome, Outcome::CancelRead | Outcome::Refused),
            "{label}"
        );
        let (class, code, detail) = match outcome {
            Outcome::Failed | Outcome::CancelAfterFailed => {
                (ErrorClass::Failed, "WriteFailed", "is ERROR")
            }
            Outcome::Timeout
            | Outcome::LongId
            | Outcome::Collision
            | Outcome::TimeoutAfterFailed => {
                assert!(error.has_identity(ErrorClass::DeadlineExceeded), "{label}");
                assert!(message.contains("context deadline exceeded"), "{label}");
                if write == Write::Create {
                    assert_eq!(
                        error.recovery_action(),
                        Some(RecoveryAction::DoNotRepeatVolumeCreate),
                        "{label}"
                    );
                    let guidance = if outcome == Outcome::Collision {
                        "Creation was accepted. Do not retry."
                    } else {
                        "The volume exists and this create must not be repeated."
                    };
                    assert_eq!(error.recovery_diagnostic(), Some(guidance), "{label}");
                    assert!(message.ends_with(guidance), "{label}");
                }
                (ErrorClass::NotSettled, "NotSettled", write.timeout_reason())
            }
            Outcome::CallerDeadline => {
                assert!(error.has_identity(ErrorClass::DeadlineExceeded), "{label}");
                (
                    ErrorClass::NotSettled,
                    "NotSettled",
                    "context deadline exceeded",
                )
            }
            Outcome::CancelRead => {
                assert!(error.has_identity(ErrorClass::Cancelled), "{label}");
                assert_eq!(
                    error.api_error().unwrap().operation(),
                    "volume.GetVolume",
                    "{label}"
                );
                (
                    ErrorClass::NotSettled,
                    "NotSettled",
                    "volume.GetVolume: request failed: canceled",
                )
            }
            Outcome::Refused => {
                let api = error.api_error().unwrap();
                assert_eq!(
                    (api.operation(), api.status(), api.code()),
                    ("volume.GetVolume", Some(403), "Forbidden"),
                    "{label}"
                );
                (
                    ErrorClass::NotSettled,
                    "NotSettled",
                    "volume.GetVolume: confirmation refused by provider (status 403)",
                )
            }
            Outcome::CancelSleep | Outcome::CancelAfterSettled => {
                assert!(error.has_identity(ErrorClass::Cancelled), "{label}");
                (ErrorClass::NotSettled, "NotSettled", "context canceled")
            }
            _ => unreachable!(),
        };
        assert_eq!(error.class(), class, "{label}");
        assert!(error.has_identity(class), "{label}");
        assert_eq!(envelope["error"]["code"], code, "{label}");
        assert!(envelope["error"].get("operation").is_none(), "{label}");
        assert!(envelope["error"].get("status").is_none(), "{label}");
        assert!(message.contains(detail), "{label}");

        if outcome != Outcome::LongId {
            let op = write.operation();
            let mut expected = match outcome {
                Outcome::Failed | Outcome::CancelAfterFailed => {
                    format!("vngcloud: write failed: {op}: volume volume-1 is ERROR")
                }
                Outcome::Timeout | Outcome::Collision | Outcome::TimeoutAfterFailed => format!(
                    "vngcloud: write outcome not confirmed: {op}: volume volume-1 {detail}\ncontext deadline exceeded"
                ),
                _ => format!(
                    "vngcloud: write outcome not confirmed: {op}: volume volume-1: {detail}"
                ),
            };
            if write == Write::Create {
                let guidance = match outcome {
                    Outcome::Timeout | Outcome::TimeoutAfterFailed => {
                        "The volume exists and this create must not be repeated."
                    }
                    Outcome::Collision => "Creation was accepted. Do not retry.",
                    _ => "Check for an existing volume with the exact name before retrying.",
                };
                expected.push('\n');
                expected.push_str(guidance);
            }
            assert!(
                message == expected,
                "{label}: unexpected settlement diagnostic"
            );
        }
    }
}

macro_rules! cases {
    ($($name:ident => $outcome:ident),+ $(,)?) => {$(
        #[tokio::test]
        async fn $name() {
            exercise(Outcome::$outcome).await;
        }
    )+};
}

cases! {
    accepts_no_wait => Accepted,
    confirms_terminal_success => Settled,
    keeps_error_volume => Failed,
    confirms_timeout_after_sleep => Timeout,
    preserves_caller_deadline => CallerDeadline,
    preserves_cancellation_during_sleep => CancelSleep,
    preserves_cancellation_during_read => CancelRead,
    retains_confirmation_refusal => Refused,
    prefers_observed_error_over_racing_cancel => CancelAfterFailed,
    retains_confirmed_volume_on_racing_cancel => CancelAfterSettled,
    prefers_internal_timeout_over_racing_error => TimeoutAfterFailed,
    retains_timeout_guidance_with_long_id => LongId,
    retains_timeout_guidance_with_collision => Collision,
}
