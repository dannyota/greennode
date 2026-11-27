use super::{
    Adapter, ExecutionResult, Format, FutureResult, LocalError, OutputView, ParsedInput,
    run_with_clock,
};
use crate::cli::input::{self, InputDescriptor, ParsedOperationInput};
use crate::cli::op::{Kind, SelectedOperation};
use crate::cli::{Invocation, Stdin, errors, password, render};
use clap::ArgMatches;
use greennode_sdk::{Clock, Config, ConfigOptions, ConfigSource, Error, RequestContext};
use std::{ffi::OsStr, io::Write};

type ManagementOperation<T> =
    for<'a> fn(&'a RequestContext, &'a Config, &'a T) -> FutureResult<'a, ExecutionResult>;

struct ManagementAdapter<'a, T> {
    operation: ManagementOperation<T>,
    options: &'a ConfigOptions,
    matches: &'a ArgMatches,
    source: &'a dyn ConfigSource,
    view: OutputView,
    error_rules: &'static [errors::Rule],
    file_input: Option<FileInput<'a, T>>,
    write_policy: WritePolicy<T>,
}

struct FileInput<'a, T> {
    stdin: &'a mut dyn Stdin,
    terminal: bool,
    clock: &'a dyn Clock,
    bind: FileBinding<T>,
}

pub(crate) type InputPreparation<T> = for<'a> fn(
    &'a RequestContext,
    &'a dyn Clock,
    &'a ArgMatches,
    &'a mut dyn Stdin,
    bool,
    &'a mut T,
) -> FutureResult<'a, Result<(), LocalError>>;

enum FileBinding<T> {
    Password(fn(&mut T, greennode_sdk::Secret)),
    Custom(InputPreparation<T>),
}

pub(crate) type ErrorWriter = fn(&mut dyn Write, &errors::Envelope<'_>, Option<&Error>);
pub(crate) type WriteFinisher = fn(&mut dyn Write, &mut dyn Write, Format, ExecutionResult) -> u8;
pub(crate) struct WritePolicy<T> {
    pub(crate) warning: Option<fn(&T) -> Option<&'static str>>,
    pub(crate) preflight: Option<fn(&errors::Envelope<'_>) -> Option<render::Projected>>,
    pub(crate) error_writer: Option<ErrorWriter>,
    pub(crate) finish: Option<WriteFinisher>,
}
impl<T> Default for WritePolicy<T> {
    fn default() -> Self {
        Self {
            warning: None,
            preflight: None,
            error_writer: None,
            finish: None,
        }
    }
}

struct ManagementInput<'a, T: 'static> {
    descriptor: &'static InputDescriptor<T>,
    check_json: Option<input::JsonChecks>,
    matches: &'a ArgMatches,
    source: &'a dyn ConfigSource,
    operation: &'static crate::cli::op::Operation,
    confirmation: Option<&'static str>,
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
            let check_json = self.check_json;
            let json = matches
                .try_get_one::<String>("cli-input-json")
                .ok()
                .flatten();
            let project = string(matches, "project-id");
            let merged = if let Some(json) = json.filter(|json| json.starts_with("file://")) {
                let (matches, json) = (matches.clone(), json.clone());
                tokio::task::spawn_blocking(move || {
                    input::merge_checked(
                        descriptor,
                        &matches,
                        Some(&json),
                        project.as_deref(),
                        check_json,
                    )
                })
                .await
                .map_err(|_| LocalError::usage("--cli-input-json: cannot read file"))?
            } else {
                input::merge_checked(
                    descriptor,
                    matches,
                    json.map(String::as_str),
                    project.as_deref(),
                    check_json,
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
            self.operation.kind,
            Kind::Write | Kind::DestructiveWrite | Kind::WriteReplacement
        )
    }

    fn needs_confirmation(&self) -> bool {
        match self.operation.kind {
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
        if let Some(message) = self.confirmation {
            return Some(message.to_owned());
        }
        if self.operation.kind != Kind::DestructiveWrite {
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

    fn before_send_warning(&self, input: &ManagementInput<'_, T>) -> Option<&'static str> {
        let warn = self.write_policy.warning?;
        input.value.as_ref().and_then(|input| warn(&input.value))
    }
    fn preflight_output(
        &self,
        _: &ManagementInput<'_, T>,
        error: &LocalError,
    ) -> Option<render::Projected> {
        self.settings_refusal_output(&errors::Envelope {
            code: error.code,
            message: &error.message,
            status: None,
            operation: None,
        })
    }
    fn settings_refusal_output(
        &self,
        envelope: &errors::Envelope<'_>,
    ) -> Option<render::Projected> {
        self.write_policy
            .preflight
            .and_then(|project| project(envelope))
    }
    fn finish_write(&self) -> Option<WriteFinisher> {
        self.write_policy.finish
    }
    fn report_error(
        &self,
        stderr: &mut dyn Write,
        envelope: &errors::Envelope<'_>,
        error: Option<&Error>,
    ) {
        if let Some(writer) = self.write_policy.error_writer {
            writer(stderr, envelope, error);
        } else {
            errors::write_error(stderr, envelope);
        }
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

    fn prepare<'a>(
        &'a mut self,
        ctx: &'a RequestContext,
        input: &'a mut ManagementInput<'_, T>,
    ) -> FutureResult<'a, Result<(), LocalError>> {
        Box::pin(async {
            if let Some(files) = &mut self.file_input {
                let input = input.value.as_mut().ok_or_else(LocalError::missing_input)?;
                match files.bind {
                    FileBinding::Password(bind) => {
                        if let Some(path) = password::path(self.matches) {
                            let secret =
                                password::read(ctx, files.clock, path, files.stdin, files.terminal)
                                    .await?;
                            bind(&mut input.value, secret);
                        }
                    }
                    FileBinding::Custom(prepare) => {
                        prepare(
                            ctx,
                            files.clock,
                            self.matches,
                            files.stdin,
                            files.terminal,
                            &mut input.value,
                        )
                        .await?
                    }
                }
            }
            Ok(())
        })
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
    management_custom(
        invocation, selected, matches, descriptor, execute, view, None,
    )
    .await
}

pub(crate) async fn management_custom<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    view: OutputView,
    check_json: Option<input::JsonChecks>,
) -> u8 {
    run_management(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        Hooks {
            view,
            check_json,
            bind: None,
            confirmation: None,
            write_policy: WritePolicy::default(),
        },
    )
    .await
}

pub(crate) async fn management_with_password<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    bind: fn(&mut T, greennode_sdk::Secret),
) -> u8 {
    run_management(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        Hooks {
            view: OutputView::default(),
            check_json: None,
            bind: Some(FileBinding::Password(bind)),
            confirmation: None,
            write_policy: WritePolicy::default(),
        },
    )
    .await
}

pub(crate) async fn management_with_input_preparation<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    prepare: InputPreparation<T>,
) -> u8 {
    run_management(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        Hooks {
            view: OutputView::default(),
            check_json: None,
            bind: Some(FileBinding::Custom(prepare)),
            confirmation: None,
            write_policy: WritePolicy::default(),
        },
    )
    .await
}

pub(crate) async fn management_with_confirmation<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    confirmation: &'static str,
) -> u8 {
    run_management(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        Hooks {
            view: OutputView::default(),
            check_json: None,
            bind: None,
            confirmation: Some(confirmation),
            write_policy: WritePolicy::default(),
        },
    )
    .await
}

pub(crate) async fn management_with_write_policy<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    presentation: (OutputView, Option<input::JsonChecks>, WritePolicy<T>),
) -> u8 {
    let (view, check_json, write_policy) = presentation;
    run_management(
        invocation,
        selected,
        matches,
        descriptor,
        execute,
        Hooks {
            view,
            check_json,
            bind: None,
            confirmation: None,
            write_policy,
        },
    )
    .await
}

struct Hooks<T> {
    view: OutputView,
    check_json: Option<input::JsonChecks>,
    bind: Option<FileBinding<T>>,
    confirmation: Option<&'static str>,
    write_policy: WritePolicy<T>,
}

async fn run_management<T: Send + 'static>(
    invocation: &mut Invocation<'_>,
    selected: SelectedOperation,
    matches: &ArgMatches,
    descriptor: &'static InputDescriptor<T>,
    execute: ManagementOperation<T>,
    hooks: Hooks<T>,
) -> u8 {
    let operation = selected.operation;
    let mut input = ManagementInput {
        descriptor,
        check_json: hooks.check_json,
        matches,
        source: invocation.source,
        operation,
        confirmation: hooks.confirmation,
        value: None,
    };
    let mut adapter = ManagementAdapter {
        operation: execute,
        options: &invocation.config_options,
        matches,
        source: invocation.source,
        view: hooks.view,
        error_rules: selected.error_rules,
        write_policy: hooks.write_policy,
        file_input: hooks.bind.map(|bind| FileInput {
            stdin: invocation.stdin,
            terminal: invocation.stdin_terminal,
            clock: invocation.clock,
            bind,
        }),
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
