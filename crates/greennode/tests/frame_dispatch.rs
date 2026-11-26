use std::cell::RefCell;
use std::rc::Rc;

use greennode::cli::dispatch::{
    self, Adapter, FutureResult, LocalError, OperationFailure, ParsedInput,
};
use greennode::cli::render;
use greennode_sdk::{Error, RequestContext};

#[derive(Default)]
struct Input {
    events: Rc<RefCell<Vec<&'static str>>>,
    write: bool,
    read_only: bool,
    yes: bool,
    query: Option<&'static str>,
}
impl ParsedInput for Input {
    fn before_input(&mut self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("before");
        Ok(())
    }
    fn merge(&mut self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("merge");
        Ok(())
    }
    fn input_guards(&mut self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("guard");
        Ok(())
    }
    fn required(&self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("required");
        Ok(())
    }
    fn is_write(&self) -> bool {
        self.write
    }
    fn needs_confirmation(&self) -> bool {
        self.write
    }
    fn yes(&self) -> bool {
        self.yes
    }
    fn read_only(&self) -> bool {
        self.read_only
    }
    fn query(&self) -> Option<&str> {
        self.query
    }
    fn output(&self) -> Option<&str> {
        None
    }
}
struct Stub {
    events: Rc<RefCell<Vec<&'static str>>>,
    fail: bool,
}
impl Adapter<Input> for Stub {
    type Settings = ();
    fn settings<'a>(&'a mut self, _: &'a RequestContext) -> FutureResult<'a, Result<(), Error>> {
        self.events.borrow_mut().push("settings");
        Box::pin(async { Ok(()) })
    }
    fn output<'a>(&self, _: &'a ()) -> Option<&'a str> {
        None
    }
    fn profile_read_only(&self, _: &()) -> bool {
        false
    }
    fn execute<'a>(
        &'a mut self,
        _: &'a RequestContext,
        _: &'a (),
        _: &'a Input,
    ) -> FutureResult<'a, Result<Option<render::Projected>, OperationFailure>> {
        self.events.borrow_mut().push("execute");
        let fail = self.fail;
        Box::pin(async move {
            let value = render::project(&serde_json::json!({"Items": [1]})).unwrap();
            if fail {
                Err(OperationFailure {
                    error: Error::provider_failed(),
                    partial: Some(value),
                    dispatch: greennode_sdk::WriteDispatch::NotAWrite,
                    query_partial: false,
                })
            } else {
                Ok(Some(value))
            }
        })
    }
    fn advice(&self, _: &Error) -> Option<&'static str> {
        Some("fixed recovery advice")
    }
}
fn invoke(mut input: Input, fail: bool) -> (u8, Vec<u8>, Vec<u8>, Vec<&'static str>) {
    let events = input.events.clone();
    let mut adapter = Stub {
        events: events.clone(),
        fail,
    };
    let (ctx, _) = RequestContext::new();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = runtime.block_on(dispatch::run(
        &mut input,
        &mut adapter,
        &ctx,
        &mut out,
        &mut err,
    ));
    let recorded = events.borrow().clone();
    (status, out, err, recorded)
}
#[test]
fn invalid_query_refuses_before_settings_and_execution() {
    let (status, out, _, events) = invoke(
        Input {
            write: true,
            yes: true,
            query: Some("[a b]"),
            ..Default::default()
        },
        false,
    );
    assert_eq!(status, 2);
    assert!(out.is_empty());
    assert_eq!(events, ["before", "merge", "guard", "required"]);
}
#[test]
fn partial_output_precedes_error_and_ignores_query() {
    let (status, out, err, events) = invoke(
        Input {
            query: Some("missing"),
            ..Default::default()
        },
        true,
    );
    assert_eq!(status, 3);
    assert_eq!(out, b"{\n  \"Items\": [\n    1\n  ]\n}\n");
    assert_eq!(
        err,
        b"{\"error\":{\"code\":\"LoginFailed\",\"message\":\"fixed recovery advice\"}}\n"
    );
    assert_eq!(
        events,
        [
            "before", "merge", "guard", "required", "settings", "execute"
        ]
    );
}
#[test]
fn query_failure_after_write_reports_no_retry() {
    let (status, out, err, events) = invoke(
        Input {
            write: true,
            yes: true,
            query: Some("unknown_function(@)"),
            ..Default::default()
        },
        false,
    );
    assert_eq!(status, 1);
    assert!(out.is_empty());
    assert_eq!(err, b"{\"error\":{\"code\":\"QueryFailed\",\"message\":\"query failed after the write succeeded; do not retry\"}}\n");
    assert_eq!(
        events.iter().filter(|event| **event == "execute").count(),
        1
    );
}
#[test]
fn read_only_precedes_required_and_settings() {
    let (status, out, _, events) = invoke(
        Input {
            write: true,
            read_only: true,
            ..Default::default()
        },
        false,
    );
    assert_eq!(status, 2);
    assert!(out.is_empty());
    assert_eq!(events, ["before", "merge", "guard"]);
}

#[test]
fn first_api_detail_controls_status_exit_unless_nested_identity_matches() {
    use greennode::cli::errors::classify;
    use greennode_sdk::{ErrorClass, testing};
    let error = testing::cause(
        testing::api("outer", Some(500), "", "failure"),
        testing::api("inner", Some(404), "", "missing"),
    );
    let result = classify(&error, &[]);
    assert_eq!(result.exit, 1);
    assert_eq!(result.envelope.status, Some(500));
    assert_eq!(result.envelope.operation, Some("outer"));
    let error = testing::cause(error, testing::identity(ErrorClass::NotFound));
    let result = classify(&error, &[]);
    assert_eq!(result.exit, 4);
    assert_eq!(result.envelope.code, "NotFound");
    assert_eq!(result.envelope.status, Some(500));
}

#[test]
fn service_rules_use_fixed_ranks_and_reject_duplicates() {
    use greennode::cli::errors::{Rule, classify, validate_rules};
    use greennode_sdk::{ErrorClass, testing};
    fn any(_: &Error) -> bool {
        true
    }
    let rule = Rule {
        identity: "quota-status",
        rank: 2,
        matches: any,
        code: "QuotaStatusIncomplete",
        exit: Some(1),
        operation: None,
        include_api_status: false,
    };
    assert!(validate_rules(&[rule]));
    assert!(!validate_rules(&[rule, rule]));
    let error = testing::cause(
        testing::identity(ErrorClass::NotSettled),
        testing::identity(ErrorClass::Auth),
    );
    let rules = [rule];
    let classified = classify(&error, &rules);
    assert_eq!(classified.envelope.code, "QuotaStatusIncomplete");
    assert_eq!(classified.exit, 1);
    let history = testing::cause(testing::identity(ErrorClass::HistoryIncomplete), error);
    assert_eq!(
        classify(&history, &[rule]).envelope.code,
        "HistoryIncomplete"
    );
}

#[test]
fn registered_stub_waits_for_cancellation_and_exits_one() {
    use greennode::cli::{Invocation, InvocationHandler, run_registered};
    use greennode_sdk::{ConfigSource, SystemClock};
    use std::ffi::{OsStr, OsString};
    struct Source;
    impl ConfigSource for Source {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
    }
    struct Waiting;
    impl Adapter<Input> for Waiting {
        type Settings = ();
        fn settings<'a>(
            &'a mut self,
            _: &'a RequestContext,
        ) -> FutureResult<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
        fn output<'a>(&self, _: &'a ()) -> Option<&'a str> {
            None
        }
        fn profile_read_only(&self, _: &()) -> bool {
            false
        }
        fn execute<'a>(
            &'a mut self,
            ctx: &'a RequestContext,
            _: &'a (),
            _: &'a Input,
        ) -> FutureResult<'a, Result<Option<render::Projected>, OperationFailure>> {
            Box::pin(async move {
                Err(OperationFailure {
                    error: ctx.wait(&SystemClock).await.into(),
                    partial: None,
                    dispatch: greennode_sdk::WriteDispatch::NotAWrite,
                    query_partial: false,
                })
            })
        }
    }
    struct Handler;
    impl InvocationHandler for Handler {
        fn accepts(&self, args: &[OsString]) -> bool {
            args == [OsString::from("greennode"), OsString::from("toy")]
        }
        fn run<'a>(
            &'a mut self,
            invocation: &'a mut Invocation<'_>,
        ) -> FutureResult<'a, std::process::ExitCode> {
            Box::pin(async move {
                let status = dispatch::run(
                    &mut Input::default(),
                    &mut Waiting,
                    invocation.context,
                    invocation.stdout,
                    invocation.stderr,
                )
                .await;
                std::process::ExitCode::from(status)
            })
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (context, cancel) = RequestContext::new();
        let (mut stdin, mut stdout, mut stderr) = (std::io::empty(), Vec::new(), Vec::new());
        let invocation = Invocation {
            args: vec!["greennode".into(), "toy".into()],
            source: &Source,
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context: &context,
        };
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });
        let status = run_registered(invocation, &mut [&mut Handler]).await;
        assert_eq!(status, std::process::ExitCode::from(1));
        assert!(stdout.is_empty());
        assert!(stderr.starts_with(b"{\"error\":"));
    });
}
