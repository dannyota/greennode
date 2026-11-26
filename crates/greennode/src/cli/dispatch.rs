mod local;
mod output;

pub use local::LocalError;
use std::ffi::OsStr;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use clap::ArgMatches;
use greennode_sdk::{Clock, Config, ConfigOptions, ConfigSource, Error, Failure, RequestContext};

use super::input::{self, InputDescriptor, ParsedOperationInput};
use super::op::{Kind, SelectedOperation};
use super::{Invocation, errors, query, render};
use output::{OutputError, render_output};
pub use output::{OutputView, RenderFailure};

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

pub type ExecutionResult =
    Result<Option<Result<render::Projected, render::Error>>, OperationFailure>;

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
    fn preflight_output(&self, _input: &I, _error: &LocalError) -> Option<render::Projected> {
        None
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

type ManagementOperation<T> =
    for<'a> fn(&'a RequestContext, &'a Config, &'a T) -> FutureResult<'a, ExecutionResult>;

struct ManagementAdapter<'a, T> {
    operation: ManagementOperation<T>,
    options: &'a ConfigOptions,
    matches: &'a ArgMatches,
    source: &'a dyn ConfigSource,
    view: OutputView,
    error_rules: &'static [errors::Rule],
}

struct ManagementInput<'a, T: 'static> {
    descriptor: &'static InputDescriptor<T>,
    matches: &'a ArgMatches,
    source: &'a dyn ConfigSource,
    kind: Kind,
    operation: &'static super::op::Operation,
    value: Option<ParsedOperationInput<T>>,
}

fn string(matches: &ArgMatches, name: &str) -> Option<String> {
    matches.get_one::<String>(name).cloned()
}

fn latch(value: Option<&str>) -> Result<bool, LocalError> {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "1" => Ok(true),
        "" | "false" | "0" => Ok(false),
        _ => Err(LocalError {
            code: "InvalidConfig",
            message: "invalid read-only setting".into(),
            exit: 2,
        }),
    }
}

impl<T: Send + 'static> ParsedInput for ManagementInput<'_, T> {
    fn before_input(&mut self) -> Result<(), LocalError> {
        input::before_input(
            self.descriptor,
            self.matches,
            self.matches
                .try_get_one::<String>("cli-input-json")
                .ok()
                .flatten()
                .map(String::as_str),
        )
        .map_err(|error| LocalError::usage(error.0))
    }

    fn merge(&mut self) -> FutureResult<'_, Result<(), LocalError>> {
        Box::pin(async {
            let descriptor = self.descriptor;
            let matches = self.matches;
            let json = matches
                .try_get_one::<String>("cli-input-json")
                .ok()
                .flatten();
            let project = string(matches, "project-id");
            let merged = if let Some(json) = json.filter(|json| json.starts_with("file://")) {
                let (matches, json) = (matches.clone(), json.clone());
                tokio::task::spawn_blocking(move || {
                    input::merge(descriptor, &matches, Some(&json), project.as_deref())
                })
                .await
                .map_err(|_| LocalError::usage("--cli-input-json: cannot read file"))?
            } else {
                input::merge(
                    descriptor,
                    matches,
                    json.map(String::as_str),
                    project.as_deref(),
                )
            };
            self.value = Some(merged.map_err(|error| LocalError::usage(error.0))?);
            Ok(())
        })
    }

    fn input_guards(&mut self) -> Result<(), LocalError> {
        let value = self.value.as_ref().ok_or_else(LocalError::missing_input)?;
        input::check_guards(self.descriptor, value, self.matches)
            .map_err(|error| LocalError::usage(error.0))
    }

    fn required(&self) -> Result<(), LocalError> {
        let value = self.value.as_ref().ok_or_else(LocalError::missing_input)?;
        input::check_required(self.descriptor, value).map_err(|error| {
            let flag = self
                .descriptor
                .fields
                .iter()
                .any(|field| field.flag.is_some_and(|flag| flag.name == error.0));
            let prefix = if flag { "--" } else { "" };
            LocalError::usage(format!("{prefix}{} is required", error.0))
        })
    }

    fn is_write(&self) -> bool {
        matches!(
            self.kind,
            Kind::Write | Kind::DestructiveWrite | Kind::WriteReplacement
        )
    }

    fn needs_confirmation(&self) -> bool {
        match self.kind {
            Kind::DestructiveWrite => true,
            Kind::ReadLocalReplacement | Kind::WriteReplacement => self
                .matches
                .try_get_one::<bool>("replace")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false),
            _ => false,
        }
    }

    fn confirmation_message(&self) -> Option<String> {
        if self.kind != Kind::DestructiveWrite {
            return None;
        }
        let (service, _) = self.operation.sdk_operation.split_once('.')?;
        Some(format!(
            "{service} {} is destructive; pass --yes to confirm",
            self.operation.name
        ))
    }

    fn yes(&self) -> bool {
        self.matches
            .get_one::<bool>("yes")
            .copied()
            .unwrap_or(false)
    }

    fn check_read_only(&self) -> Result<(), LocalError> {
        let on = |name| {
            let value = self.source.var_os(OsStr::new(name));
            let value = value.as_deref().map(OsStr::to_string_lossy);
            latch(value.as_deref())
        };
        let (greennode, vngcloud) = (on("GREENNODE_READ_ONLY")?, on("VNGCLOUD_READ_ONLY")?);
        if self.matches.get_one::<bool>("read-only") == Some(&true) {
            Err(LocalError::read_only("--read-only"))
        } else if greennode {
            Err(LocalError::read_only("GREENNODE_READ_ONLY"))
        } else if vngcloud {
            Err(LocalError::read_only("VNGCLOUD_READ_ONLY"))
        } else {
            Ok(())
        }
    }

    fn query(&self) -> Option<&str> {
        self.matches.get_one::<String>("query").map(String::as_str)
    }

    fn output(&self) -> Option<&str> {
        self.matches.get_one::<String>("output").map(String::as_str)
    }
}

impl<T: Send + 'static> Adapter<ManagementInput<'_, T>> for ManagementAdapter<'_, T> {
    type Settings = Config;

    fn settings<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
    ) -> FutureResult<'a, Result<Config, Error>> {
        let mut options = self.options.clone();
        let supplied = |name| string(self.matches, name).filter(|value| !value.is_empty());
        if let Some(profile) = supplied("profile") {
            options = options.with_profile(profile);
        }
        if let Some(region) = supplied("region") {
            options = options.with_region(region);
        }
        if let Some(project) = supplied("project-id") {
            options = options.with_project_id(project);
        }
        if let Some(home) = self.source.home_dir().filter(|path| path.is_absolute()) {
            options = options.with_token_cache(home.join(".greennode").join("cache"));
        }
        Box::pin(Config::load(ctx, options, self.source))
    }

    fn output<'a>(&self, settings: &'a Config) -> Option<&'a str> {
        settings.profile_setting("output")
    }

    fn check_profile_read_only(&self, settings: &Config) -> Result<(), LocalError> {
        if latch(settings.profile_setting("read_only"))? {
            return Err(LocalError::read_only("config"));
        }
        Ok(())
    }

    fn output_view(&self) -> OutputView {
        self.view
    }

    fn rules(&self) -> &[errors::Rule] {
        self.error_rules
    }

    fn advice(&self, error: &Error) -> Option<&'static str> {
        error.detail::<&'static str>().copied()
    }

    fn execute<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
        settings: &'a Config,
        input: &'a ManagementInput<'_, T>,
    ) -> FutureResult<'a, ExecutionResult> {
        match &input.value {
            Some(input) => (self.operation)(ctx, settings, &input.value),
            None => Box::pin(async {
                Ok(Some(Err(render::Error::Invalid(
                    "operation input unavailable".into(),
                ))))
            }),
        }
    }
}

pub(crate) async fn management<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
) -> u8 {
    management_with_view(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        OutputView::default(),
    )
    .await
}

pub(crate) async fn management_with_view<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    view: OutputView,
) -> u8 {
    let operation = selected.operation;
    let mut input = ManagementInput {
        descriptor,
        matches,
        source: invocation.source,
        kind: operation.kind,
        operation,
        value: None,
    };
    let mut adapter = ManagementAdapter {
        operation: execute,
        options: &invocation.config_options,
        matches,
        source: invocation.source,
        view,
        error_rules: selected.error_rules,
    };
    run_with_clock(
        &mut input,
        &mut adapter,
        invocation.context,
        invocation.clock,
        invocation.stdout,
        invocation.stderr,
        operation
            .sdk_operation
            .split_once('.')
            .filter(|_| matches.get_one::<bool>("debug") == Some(&true))
            .map(|(service, _)| (service, operation.name)),
    )
    .await
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
            })
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
        let format = input
            .output()
            .and_then(Format::parse)
            .unwrap_or(Format::Json);
        let _ = render_output(stdout, &output, format, None, OutputView::default());
    }
    local(stderr, error)
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
        let classified = errors::classify(&error, adapter.rules());
        errors::write_error(stderr, &classified.envelope);
        return classified.exit;
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
            let classified = errors::classify(&error, adapter.rules());
            errors::write_error(stderr, &classified.envelope);
            return classified.exit;
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
    let view = adapter.output_view();
    let log_write = |stderr: &mut dyn Write, message| {
        if let Some((service, name)) = write_debug.filter(|_| input.is_write()) {
            let _ = writeln!(
                stderr,
                "level=DEBUG msg=\"{message}\" operation=\"{service} {name}\""
            );
        }
    };
    log_write(stderr, "write started");
    let result = adapter.execute(ctx, &settings, input).await;
    log_write(stderr, "write finished");
    match result {
        Ok(Some(output)) => match output
            .map_err(OutputError::from)
            .and_then(|output| render_output(stdout, &output, format, query.as_ref(), view))
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
        Err(failure) => {
            if let Some(partial) = &failure.partial {
                let _ = render_output(
                    stdout,
                    partial,
                    format,
                    if failure.query_partial {
                        query.as_ref()
                    } else {
                        None
                    },
                    OutputView::default(),
                );
            }
            let mut classified = errors::classify(&failure.error, adapter.rules());
            if let Some(advice) = failure.error.detail::<String>() {
                classified.envelope.message = advice;
            } else if let Some(advice) = adapter.advice(&failure.error) {
                classified.envelope.message = advice;
            }
            errors::write_error(stderr, &classified.envelope);
            classified.exit
        }
    }
}
