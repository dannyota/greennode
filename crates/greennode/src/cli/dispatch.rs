use std::future::Future;
use std::io::Write;
use std::pin::Pin;

use greennode_sdk::{Config, ConfigOptions, ConfigSource, Error, RequestContext};

use super::{errors, query, render};

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

pub struct LocalError {
    pub code: &'static str,
    pub message: &'static str,
    pub exit: u8,
}

impl LocalError {
    pub fn usage(message: &'static str) -> Self {
        Self {
            code: "InvalidUsage",
            message,
            exit: 2,
        }
    }
    pub fn read_only() -> Self {
        Self {
            code: "ReadOnly",
            message: "read-only: write refused",
            exit: 2,
        }
    }
}

pub trait ParsedInput {
    fn before_input(&mut self) -> Result<(), LocalError>;
    fn merge(&mut self) -> Result<(), LocalError>;
    fn input_guards(&mut self) -> Result<(), LocalError>;
    fn required(&self) -> Result<(), LocalError>;
    fn is_write(&self) -> bool;
    fn needs_confirmation(&self) -> bool;
    fn yes(&self) -> bool;
    fn read_only(&self) -> bool;
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
    fn profile_read_only(&self, settings: &Self::Settings) -> bool;
    fn execute<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
        settings: &'a Self::Settings,
        input: &'a I,
    ) -> FutureResult<'a, Result<Option<render::Projected>, OperationFailure>>;
    fn rules(&self) -> &[errors::Rule] {
        &[]
    }
    fn advice(&self, _error: &Error) -> Option<&'static str> {
        None
    }
}

pub struct ManagementSettings<'a> {
    pub options: ConfigOptions,
    pub source: &'a dyn ConfigSource,
}

impl ManagementSettings<'_> {
    pub async fn load(&self, ctx: &RequestContext) -> Result<Config, Box<Error>> {
        Config::load(ctx, self.options.clone(), self.source)
            .await
            .map_err(Box::new)
    }
}

fn local(stderr: &mut dyn Write, error: LocalError) -> u8 {
    errors::write_error(
        stderr,
        &errors::Envelope {
            code: error.code,
            message: error.message,
            status: None,
            operation: None,
        },
    );
    error.exit
}

enum OutputError {
    Query,
    Render(render::Error),
}

impl From<render::Error> for OutputError {
    fn from(error: render::Error) -> Self {
        Self::Render(error)
    }
}

fn render_output(
    writer: &mut dyn Write,
    projected: &render::Projected,
    format: Format,
    query: Option<&query::CompiledQuery>,
) -> Result<(), OutputError> {
    if format == Format::Json && query.is_none() {
        return render::write_json(&mut &mut *writer, projected).map_err(Into::into);
    }
    let mut value = projected.for_query()?;
    if let Some(query) = query {
        value = query::evaluate(query, &value).map_err(|_| OutputError::Query)?;
    }
    match format {
        Format::Json => render::write_json(&mut &mut *writer, &render::project(&value)?),
        Format::Table => render::write_table(&mut &mut *writer, &value),
        Format::Text => render::write_text(&mut &mut *writer, &value),
    }
    .map_err(Into::into)
}

pub async fn run<I: ParsedInput, A: Adapter<I>>(
    input: &mut I,
    adapter: &mut A,
    ctx: &RequestContext,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    if !errors::validate_rules(adapter.rules()) {
        return local(
            stderr,
            LocalError {
                code: "RequestFailed",
                message: "invalid error rules",
                exit: 1,
            },
        );
    }
    for check in [I::before_input, I::merge, I::input_guards] {
        if let Err(error) = check(input) {
            return local(stderr, error);
        }
    }
    if input.is_write() && input.read_only() {
        return local(stderr, LocalError::read_only());
    }
    if let Err(error) = input.required() {
        return local(stderr, error);
    }
    let query = match input.query().filter(|value| !value.is_empty()) {
        Some(value) => match query::compile(value) {
            Ok(query) => Some(query),
            Err(_) => return local(stderr, LocalError::usage("invalid query")),
        },
        None => None,
    };
    let explicit_format = match input.output().filter(|value| !value.is_empty()) {
        Some(value) => match Format::parse(value) {
            Some(format) => Some(format),
            None => return local(stderr, LocalError::usage("invalid output format")),
        },
        None => None,
    };
    if input.needs_confirmation() && !input.yes() {
        return local(stderr, LocalError::usage("confirmation requires --yes"));
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
        None => return local(stderr, LocalError::usage("invalid output format")),
    };
    if input.is_write() && adapter.profile_read_only(&settings) {
        return local(stderr, LocalError::read_only());
    }
    match adapter.execute(ctx, &settings, input).await {
        Ok(Some(output)) => match render_output(stdout, &output, format, query.as_ref()) {
            Ok(()) => 0,
            Err(error) => {
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
                        message,
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
                );
            }
            let mut classified = errors::classify(&failure.error, adapter.rules());
            if let Some(advice) = adapter.advice(&failure.error) {
                classified.envelope.message = advice;
            }
            errors::write_error(stderr, &classified.envelope);
            classified.exit
        }
    }
}
