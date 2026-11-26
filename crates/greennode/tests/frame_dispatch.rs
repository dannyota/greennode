use std::cell::RefCell;
use std::rc::Rc;

use greennode::cli::dispatch::{
    self, Adapter, ExecutionResult, FutureResult, LocalError, OperationFailure, ParsedInput,
    RenderFailure,
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
    reject: bool,
    billing: bool,
}
impl ParsedInput for Input {
    fn before_input(&mut self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("before");
        Ok(())
    }
    fn merge(&mut self) -> FutureResult<'_, Result<(), LocalError>> {
        self.events.borrow_mut().push("merge");
        Box::pin(async { Ok(()) })
    }
    fn input_guards(&mut self) -> Result<(), LocalError> {
        self.events.borrow_mut().push("guard");
        if self.reject {
            return Err(LocalError::usage("query is not allowed"));
        }
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
    fn check_read_only(&self) -> Result<(), LocalError> {
        if self.read_only {
            return Err(LocalError::read_only("--read-only"));
        }
        Ok(())
    }
    fn query(&self) -> Option<&str> {
        self.query
    }
    fn output(&self) -> Option<&str> {
        self.billing.then_some("table")
    }
}
struct Stub {
    events: Rc<RefCell<Vec<&'static str>>>,
    fail: bool,
    billing: bool,
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
    fn check_profile_read_only(&self, _: &()) -> Result<(), LocalError> {
        Ok(())
    }
    fn execute<'a>(
        &'a mut self,
        _: &'a RequestContext,
        _: &'a (),
        _: &'a Input,
    ) -> FutureResult<'a, ExecutionResult> {
        self.events.borrow_mut().push("execute");
        let fail = self.fail;
        let billing = self.billing;
        Box::pin(async move {
            let value = if billing {
                let fields: std::collections::BTreeMap<String, u8> =
                    (0..101).map(|n| (format!("column{n}"), 1)).collect();
                render::project(&fields).unwrap()
            } else {
                render::project(&serde_json::json!({"Items": [1]})).unwrap()
            };
            if fail {
                Err(OperationFailure {
                    error: Error::provider_failed(),
                    partial: Some(value),
                    dispatch: greennode_sdk::WriteDispatch::NotAWrite,
                    query_partial: false,
                })
            } else {
                Ok(Some(Ok(value)))
            }
        })
    }
    fn preflight_output(&self, input: &Input, _: &LocalError) -> Option<render::Projected> {
        input
            .reject
            .then(|| render::project(&serde_json::json!({"Effect": "not sent"})).unwrap())
    }
    fn render_failure(&self, failure: RenderFailure) -> Option<LocalError> {
        (self.billing && failure == RenderFailure::TableLimit).then_some(LocalError {
            code: "QueryFailed",
            message: "billing report withheld".into(),
            exit: 1,
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
        billing: input.billing,
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
fn preflight_refusal_emits_safe_outcome_before_error_without_settings() {
    #[derive(Clone)]
    struct Writer(&'static str, Rc<RefCell<Vec<&'static str>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.1.borrow_mut().push(self.0);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut input = Input {
        reject: true,
        query: Some(""),
        events: events.clone(),
        ..Default::default()
    };
    let mut adapter = Stub {
        events: events.clone(),
        fail: false,
        billing: false,
    };
    let (ctx, _) = RequestContext::new();
    let mut out = Writer("stdout", events.clone());
    let mut err = Writer("stderr", events.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(dispatch::run(
            &mut input,
            &mut adapter,
            &ctx,
            &mut out,
            &mut err
        )),
        2
    );
    let events = events.borrow();
    assert_eq!(&events[..3], ["before", "merge", "guard"]);
    let first_err = events.iter().position(|event| *event == "stderr").unwrap();
    assert!(events[3..first_err].iter().all(|event| *event == "stdout"));
    assert!(first_err > 3);
    assert!(events[first_err..].iter().all(|event| *event == "stderr"));
    let (status, out, err, _) = invoke(
        Input {
            reject: true,
            query: Some(""),
            ..Default::default()
        },
        false,
    );
    assert_eq!(status, 2);
    assert_eq!(out, b"{\n  \"Effect\": \"not sent\"\n}\n");
    assert_eq!(
        err,
        b"{\"error\":{\"code\":\"InvalidUsage\",\"message\":\"query is not allowed\"}}\n"
    );
}

#[test]
fn cancellation_before_dispatch_skips_guards_and_settings() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut input = Input {
        events: events.clone(),
        ..Default::default()
    };
    let mut adapter = Stub {
        events: events.clone(),
        fail: false,
        billing: false,
    };
    let (ctx, cancel) = RequestContext::new();
    cancel.cancel();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(dispatch::run(
            &mut input,
            &mut adapter,
            &ctx,
            &mut out,
            &mut err
        )),
        1
    );
    assert!(events.borrow().is_empty());
    assert!(out.is_empty());
    assert!(!err.is_empty());
}

#[test]
fn billing_policy_withholds_table_limit_output() {
    let (status, out, err, events) = invoke(
        Input {
            billing: true,
            ..Default::default()
        },
        false,
    );
    assert_eq!(status, 1);
    assert!(out.is_empty());
    assert_eq!(
        err,
        b"{\"error\":{\"code\":\"QueryFailed\",\"message\":\"billing report withheld\"}}\n"
    );
    assert_eq!(
        events.iter().filter(|event| **event == "execute").count(),
        1
    );
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
    use greennode::cli::errors::{
        Rule, RuleExit, RuleName, RuleOperation, classify, validate_rules,
    };
    use greennode_sdk::{ErrorClass, testing};
    fn any(_: &Error) -> bool {
        true
    }
    let rule = Rule {
        message: None,
        identity: "quota-status",
        matches: any,
        name: Some(RuleName {
            rank: 2,
            code: "QuotaStatusIncomplete",
            operation: RuleOperation::Api,
            include_api_status: true,
        }),
        exit: Some(RuleExit::Failure),
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
    assert_eq!(classified.envelope.operation, None);
    assert_eq!(classified.envelope.status, None);
    let history = testing::cause(testing::identity(ErrorClass::HistoryIncomplete), error);
    assert_eq!(
        classify(&history, &[rule]).envelope.code,
        "HistoryIncomplete"
    );
}

#[test]
fn page_format_name_follows_api_and_invalid_config() {
    use greennode::cli::errors::{
        PAGE_FORMAT_RANK, Rule, RuleName, RuleOperation, classify, validate_rules,
    };
    use greennode_sdk::{ErrorClass, testing};

    let rules = [Rule {
        message: None,
        identity: "page-format",
        matches: |error| error.has_identity(ErrorClass::RateLimited),
        name: Some(RuleName {
            rank: PAGE_FORMAT_RANK,
            code: "PageFormat",
            operation: RuleOperation::Omit,
            include_api_status: false,
        }),
        exit: None,
    }];
    let page_format = || testing::identity(ErrorClass::RateLimited);
    let api = testing::cause(
        page_format(),
        testing::api("list", Some(500), "ServiceFailure", "failure"),
    );
    let classified = classify(&api, &rules);
    assert_eq!(classified.envelope.code, "ServiceFailure");
    assert_eq!(classified.envelope.status, Some(500));
    assert_eq!(classified.envelope.operation, Some("list"));

    let config = testing::cause(page_format(), testing::identity(ErrorClass::InvalidConfig));
    let classified = classify(&config, &rules);
    assert_eq!(classified.envelope.code, "InvalidConfig");
    assert_eq!(classified.exit, 2);

    assert!(validate_rules(&rules));
    assert_eq!(classify(&page_format(), &rules).envelope.code, "PageFormat");
}

#[test]
fn dispatched_operation_waits_for_cancellation_and_exits_one() {
    use greennode_sdk::SystemClock;
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
        fn check_profile_read_only(&self, _: &()) -> Result<(), LocalError> {
            Ok(())
        }
        fn execute<'a>(
            &'a mut self,
            ctx: &'a RequestContext,
            _: &'a (),
            _: &'a Input,
        ) -> FutureResult<'a, ExecutionResult> {
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (context, cancel) = RequestContext::new();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });
        let status = dispatch::run(
            &mut Input::default(),
            &mut Waiting,
            &context,
            &mut stdout,
            &mut stderr,
        )
        .await;
        assert_eq!(status, 1);
        assert!(stdout.is_empty());
        assert!(stderr.starts_with(b"{\"error\":"));
    });
}

#[test]
fn fixed_rule_message_overrides_api_message() {
    use greennode::cli::errors::{Rule, classify};
    use greennode_sdk::{Error, testing};
    let rules = [Rule {
        identity: "fixed",
        matches: |_: &Error| true,
        name: None,
        exit: None,
        message: Some("fixed advice"),
    }];
    let error = testing::api("operation", Some(500), "Failed", "inner message");
    assert_eq!(classify(&error, &rules).envelope.message, "fixed advice");
}
