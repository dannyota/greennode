mod local;
mod management;
mod output;

pub use local::LocalError;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use greennode_sdk::{Clock, Error, Failure, RequestContext};

use super::{errors, query, render};
pub(crate) use management::{
    ErrorWriter, WritePolicy, management, management_custom, management_with_confirmation,
    management_with_input_preparation, management_with_password, management_with_view,
    management_with_write_policy,
};
pub(crate) use output::RawText;
pub use output::{ExecutionFailure, ExecutionResult, OutputView, RenderFailure};
use output::{OutputError, render_output};

pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Table,
    Text,
}

impl Format {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "json" => Some(Self::Json),
            "table" => Some(Self::Table),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

pub trait ParsedInput {
    fn before_input(&mut self) -> Result<(), LocalError>;
    fn merge(&mut self) -> FutureResult<'_, Result<(), LocalError>>;
    fn input_guards(&mut self) -> Result<(), LocalError>;
    fn required(&self) -> Result<(), LocalError>;
    fn is_write(&self) -> bool;
    fn needs_confirmation(&self) -> bool;
    fn confirmation_message(&self) -> Option<String> {
        None
    }
    fn yes(&self) -> bool;
    fn check_read_only(&self) -> Result<(), LocalError>;
    fn query(&self) -> Option<&str>;
    fn output(&self) -> Option<&str>;
}

pub struct OperationFailure {
    pub error: Error,
    pub partial: Option<render::Projected>,
    pub dispatch: greennode_sdk::WriteDispatch,
    pub query_partial: bool,
}

pub trait Adapter<I: ParsedInput> {
    type Settings;
    fn settings<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
    ) -> FutureResult<'a, Result<Self::Settings, Error>>;
    fn output<'a>(&self, settings: &'a Self::Settings) -> Option<&'a str>;
    fn check_profile_read_only(&self, settings: &Self::Settings) -> Result<(), LocalError>;
    fn execute<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
        settings: &'a Self::Settings,
        input: &'a I,
    ) -> FutureResult<'a, ExecutionResult>;
    fn prepare<'a>(
        &'a mut self,
        _ctx: &'a RequestContext,
        _input: &'a mut I,
    ) -> FutureResult<'a, Result<(), LocalError>> {
        Box::pin(async { Ok(()) })
    }
    fn preflight_output(&self, _input: &I, _error: &LocalError) -> Option<render::Projected> {
        None
    }
    fn before_send_warning(&self, _input: &I) -> Option<&'static str> {
        None
    }
    fn settings_refusal_output(
        &self,
        _input: &I,
        _error: &LocalError,
    ) -> Option<render::Projected> {
        None
    }
    fn report_error(
        &self,
        stderr: &mut dyn Write,
        envelope: &errors::Envelope<'_>,
        _error: Option<&Error>,
    ) {
        errors::write_error(stderr, envelope);
    }
    fn output_view(&self) -> OutputView {
        OutputView::default()
    }
    fn render_failure(&self, _failure: RenderFailure) -> Option<LocalError> {
        None
    }
    fn rules(&self) -> &[errors::Rule] {
        &[]
    }
    fn advice(&self, _error: &Error) -> Option<&'static str> {
        None
    }
}

#[expect(
    clippy::result_large_err,
    reason = "OperationFailure retains the SDK error and partial output"
)]
pub(crate) fn project_result<T, P: serde::Serialize>(
    outcome: Result<T, Failure<T>>,
    project: impl Fn(T) -> P,
) -> ExecutionResult {
    match outcome {
        Ok(output) => Ok(Some(render::project(&project(output)))),
        Err(failure) => {
            let (error, partial, dispatch) = failure.into_parts();
            Err(OperationFailure {
                error,
                partial: partial.and_then(|partial| render::project(&project(partial)).ok()),
                dispatch,
                query_partial: false,
            }
            .into())
        }
    }
}

pub(super) fn local(stderr: &mut dyn Write, error: LocalError) -> u8 {
    errors::write_error(
        stderr,
        &errors::Envelope {
            code: error.code,
            message: &error.message,
            status: None,
            operation: None,
        },
    );
    error.exit
}

fn refuse<I: ParsedInput, A: Adapter<I>>(
    input: &I,
    adapter: &A,
    error: LocalError,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    if let Some(output) = adapter.preflight_output(input, &error) {
        write_preflight(stdout, stderr, &output, input.output());
    }
    report_local::<I, A>(adapter, stderr, error)
}

pub(crate) fn write_preflight(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    output: &render::Projected,
    format: Option<&str>,
) {
    let format = format.and_then(Format::parse).unwrap_or(Format::Json);
    let _ = render_output(stdout, stderr, output, format, None, OutputView::default());
}

fn report_local<I: ParsedInput, A: Adapter<I>>(
    adapter: &A,
    stderr: &mut dyn Write,
    error: LocalError,
) -> u8 {
    adapter.report_error(
        stderr,
        &errors::Envelope {
            code: error.code,
            message: &error.message,
            status: None,
            operation: None,
        },
        None,
    );
    error.exit
}

fn settings_error<I: ParsedInput, A: Adapter<I>>(
    input: &I,
    adapter: &A,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    error: &Error,
) -> u8 {
    let classified = errors::classify(error, adapter.rules());
    let code = match classified.envelope.code {
        "NoCredentials" => "NoCredentials",
        "TokenCacheFailed" => "TokenCacheFailed",
        "InvalidConfig" => "InvalidConfig",
        "InvalidUsage" => "InvalidUsage",
        "ServiceAccountLoginFailed" => "ServiceAccountLoginFailed",
        "LoginFailed" => "LoginFailed",
        _ => "RequestFailed",
    };
    let local = LocalError {
        code,
        message: classified.envelope.message.to_owned().into(),
        exit: classified.exit,
    };
    if let Some(output) = adapter.settings_refusal_output(input, &local) {
        write_preflight(stdout, stderr, &output, input.output());
    }
    adapter.report_error(stderr, &classified.envelope, Some(error));
    classified.exit
}

pub async fn run<I: ParsedInput, A: Adapter<I>>(
    input: &mut I,
    adapter: &mut A,
    ctx: &RequestContext,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    run_with_clock(
        input,
        adapter,
        ctx,
        &greennode_sdk::SystemClock,
        stdout,
        stderr,
        None,
    )
    .await
}

async fn run_with_clock<I: ParsedInput, A: Adapter<I>>(
    input: &mut I,
    adapter: &mut A,
    ctx: &RequestContext,
    clock: &dyn Clock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    write_debug: Option<(&'static str, &'static str)>,
) -> u8 {
    if let Some(end) = ctx.check(clock) {
        let error = Error::from(end);
        return settings_error(input, adapter, stdout, stderr, &error);
    }
    if !errors::validate_rules(adapter.rules()) {
        return local(
            stderr,
            LocalError {
                code: "RequestFailed",
                message: "invalid error rules".into(),
                exit: 1,
            },
        );
    }
    if let Err(error) = input.before_input() {
        return refuse(input, adapter, error, stdout, stderr);
    }
    if let Err(error) = input.merge().await {
        return refuse(input, adapter, error, stdout, stderr);
    }
    if let Err(error) = input.input_guards() {
        return refuse(input, adapter, error, stdout, stderr);
    }
    if input.is_write()
        && let Err(error) = input.check_read_only()
    {
        return refuse(input, adapter, error, stdout, stderr);
    }
    if let Err(error) = input.required() {
        return refuse(input, adapter, error, stdout, stderr);
    }
    let query = match input.query().filter(|value| !value.is_empty()) {
        Some(value) => match query::compile(value) {
            Ok(query) => Some(query),
            Err(_) => {
                return refuse(
                    input,
                    adapter,
                    LocalError::usage("invalid query"),
                    stdout,
                    stderr,
                );
            }
        },
        None => None,
    };
    let explicit_format = match input.output().filter(|value| !value.is_empty()) {
        Some(value) => match Format::parse(value) {
            Some(format) => Some(format),
            None => {
                return refuse(
                    input,
                    adapter,
                    LocalError::usage("invalid output format"),
                    stdout,
                    stderr,
                );
            }
        },
        None => None,
    };
    if input.needs_confirmation() && !input.yes() {
        return refuse(
            input,
            adapter,
            LocalError::usage(
                input
                    .confirmation_message()
                    .unwrap_or_else(|| "confirmation requires --yes".to_owned()),
            ),
            stdout,
            stderr,
        );
    }
    let settings = match adapter.settings(ctx).await {
        Ok(settings) => settings,
        Err(error) => {
            return settings_error(input, adapter, stdout, stderr, &error);
        }
    };
    let format = match explicit_format.or_else(|| {
        adapter
            .output(&settings)
            .filter(|s| !s.is_empty())
            .and_then(Format::parse)
    }) {
        Some(format) => format,
        None if adapter.output(&settings).is_none_or(str::is_empty) => Format::Json,
        None => {
            return refuse(
                input,
                adapter,
                LocalError::usage("invalid output format"),
                stdout,
                stderr,
            );
        }
    };
    if input.is_write()
        && let Err(error) = adapter.check_profile_read_only(&settings)
    {
        return refuse(input, adapter, error, stdout, stderr);
    }
    if let Err(error) = adapter.prepare(ctx, input).await {
        return refuse(input, adapter, error, stdout, stderr);
    }
    let view = adapter.output_view();
    let log_write = |stderr: &mut dyn Write, message| {
        if let Some((service, name)) = write_debug.filter(|_| input.is_write()) {
            let _ = writeln!(
                stderr,
                "level=DEBUG msg=\"{message}\" operation=\"{service} {name}\""
            );
        }
    };
    if let Some(warning) = adapter.before_send_warning(input) {
        let _ = writeln!(stderr, "{warning}");
    }
    log_write(stderr, "write started");
    let result = adapter.execute(ctx, &settings, input).await;
    log_write(stderr, "write finished");
    match result {
        Ok(Some(output)) => match output
            .map_err(OutputError::from)
            .and_then(|output| render_output(stdout, stderr, &output, format, query.as_ref(), view))
        {
            Ok(()) => 0,
            Err(error) => {
                let failure = match &error {
                    OutputError::Query => RenderFailure::Query,
                    OutputError::Render(render::Error::TableLimit) => RenderFailure::TableLimit,
                    OutputError::Render(render::Error::Io(_)) => RenderFailure::Output,
                    OutputError::Render(_) => RenderFailure::Encoding,
                };
                if failure != RenderFailure::Output
                    && let Some(message) = view.withhold
                {
                    return local(
                        stderr,
                        LocalError {
                            code: "QueryFailed",
                            message: message.into(),
                            exit: 1,
                        },
                    );
                }
                if let Some(classified) = adapter.render_failure(failure) {
                    return local(stderr, classified);
                }
                let (code, message, exit) = if matches!(error, OutputError::Query) {
                    (
                        "QueryFailed",
                        if input.is_write() {
                            "query failed after the write succeeded; do not retry"
                        } else {
                            "query failed"
                        },
                        1,
                    )
                } else if matches!(error, OutputError::Render(render::Error::TableLimit)) {
                    if input.is_write() {
                        (
                            "QueryFailed",
                            "write succeeded; do not retry; use --output json",
                            1,
                        )
                    } else {
                        (
                            "InvalidUsage",
                            "table output exceeds limits; use --output json",
                            2,
                        )
                    }
                } else {
                    (
                        "RequestFailed",
                        if input.is_write() {
                            "write succeeded; do not retry"
                        } else {
                            "output failed"
                        },
                        1,
                    )
                };
                local(
                    stderr,
                    LocalError {
                        code,
                        message: message.into(),
                        exit,
                    },
                )
            }
        },
        Ok(None) => 0,
        Err(ExecutionFailure::Local(error)) => report_local::<I, A>(adapter, stderr, error),
        Err(ExecutionFailure::Sdk(failure)) => {
            if let Some(partial) = &failure.partial {
                let _ = render_output(
                    stdout,
                    stderr,
                    partial,
                    format,
                    if failure.query_partial {
                        query.as_ref()
                    } else {
                        None
                    },
                    if failure.query_partial || view.use_for_partial {
                        view
                    } else {
                        OutputView::default()
                    },
                );
            }
            let mut classified = errors::classify(&failure.error, adapter.rules());
            if let Some(advice) = failure.error.detail::<String>() {
                classified.envelope.message = advice;
            } else if let Some(advice) = adapter.advice(&failure.error) {
                classified.envelope.message = advice;
            }
            adapter.report_error(stderr, &classified.envelope, Some(&failure.error));
            classified.exit
        }
    }
}
