use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::{Arg, ArgAction, Command};

fn command() -> Command {
    let mut root = Command::new("greennode")
        .about("Command-line access to GreenNode")
        .disable_version_flag(true)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .args_override_self(true)
        .arg(
            Arg::new("help-flag")
                .long("help")
                .short('h')
                .global(true)
                .action(ArgAction::Count),
        )
        .arg(
            Arg::new("version-flag")
                .long("version")
                .short('v')
                .action(ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("version")
                .disable_help_flag(true)
                .about("Print the greennode version"),
        )
        .subcommand(
            Command::new("configure")
                .disable_help_flag(true)
                .disable_help_subcommand(true)
                .subcommand(
                    Command::new("get")
                        .disable_help_flag(true)
                        .arg(Arg::new("key").required_unless_present("help-flag")),
                )
                .subcommand(Command::new("list").disable_help_flag(true))
                .subcommand(
                    Command::new("set")
                        .disable_help_flag(true)
                        .arg(Arg::new("key").required_unless_present("help-flag"))
                        .arg(
                            Arg::new("value")
                                .required_unless_present("help-flag")
                                .value_parser(clap::value_parser!(OsString)),
                        ),
                ),
        )
        .subcommand(
            Command::new("help")
                .disable_help_flag(true)
                .about("Print this message or the help of the given subcommand(s)")
                .arg(Arg::new("path").num_args(0..).action(ArgAction::Append)),
        )
        .subcommand(
            Command::new("gen-docs")
                .disable_help_flag(true)
                .hide(true)
                .arg(
                    Arg::new("directory")
                        .required(true)
                        .value_parser(clap::value_parser!(std::path::PathBuf)),
                ),
        )
        .subcommand(
            Command::new("__command-inventory")
                .disable_help_flag(true)
                .hide(true),
        );
    for &name in crate::services::GLOBAL_FLAGS {
        let arg = Arg::new(name).long(name).global(true);
        let arg = if matches!(name, "yes" | "read-only" | "debug") {
            arg.action(ArgAction::Set)
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("true")
                .value_parser(clap::value_parser!(bool))
        } else {
            arg.action(ArgAction::Set)
        };
        root = root.arg(arg);
    }
    root
}

pub fn command_with_registry(registry: &crate::services::Registry) -> Command {
    let mut root = command();
    for service in registry.services() {
        let mut service_command = Command::new(service.name)
            .args_override_self(true)
            .disable_help_flag(true)
            .disable_help_subcommand(true);
        for operation in service.operations {
            let mut operation_command = Command::new(operation.name)
                .disable_help_flag(true)
                .args_override_self(true)
                .about(operation.short_help);
            if !operation.notes.is_empty() {
                operation_command = operation_command.long_about(operation.notes.join("\n\n"));
            }
            if operation.handler.is_none_or(|handler| handler.allow_json) {
                operation_command = operation_command.arg(
                    Arg::new("cli-input-json")
                        .long("cli-input-json")
                        .action(ArgAction::Set),
                );
            }
            for flag in operation.flags.iter().chain(operation.extra_flags) {
                operation_command = operation_command.arg(super::op::argument(flag));
            }
            service_command = service_command.subcommand(operation_command);
        }
        root = root.subcommand(service_command);
    }
    root
}

fn collect_command_paths(command: &Command, parent: &str, paths: &mut Vec<String>) {
    let path = format!("{parent} {}", command.get_name());
    paths.push(path.clone());
    for child in command.get_subcommands().filter(|c| !c.is_hide_set()) {
        collect_command_paths(child, &path, paths);
    }
}

fn inventory(registry: &crate::services::Registry) -> String {
    let mut paths = [
        "greennode",
        "greennode --help",
        "greennode --version",
        "greennode -h",
        "greennode -v",
        "greennode help",
        "greennode version",
    ]
    .map(str::to_owned)
    .to_vec();
    let root = command_with_registry(registry);
    for service in root
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && !["version", "help"].contains(&c.get_name()))
    {
        collect_command_paths(service, "greennode", &mut paths);
    }
    paths.sort();
    let mut output = String::from("[\n");
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            output.push_str(",\n");
        }
        output.push_str("  \"");
        for c in path.chars() {
            match c {
                '"' => output.push_str("\\\""),
                '\\' => output.push_str("\\\\"),
                c if c <= '\u{1f}' => {
                    use std::fmt::Write;
                    let _ = write!(output, "\\u{:04x}", c as u32);
                }
                c => output.push(c),
            }
        }
        output.push('"');
    }
    output.push_str("\n]\n");
    output
}

fn version() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    format!("greennode {} {os}/{arch}", env!("CARGO_PKG_VERSION"))
}

fn render_help_path(registry: &crate::services::Registry, path: &[&str]) -> Option<String> {
    let mut root = command_with_registry(registry);
    root.build();
    let mut selected = &mut root;
    for name in path {
        selected = selected.find_subcommand_mut(name)?;
        if selected.is_hide_set() {
            return None;
        }
    }
    Some(if selected.get_long_about().is_some() {
        selected.render_long_help().to_string()
    } else {
        selected.render_help().to_string()
    })
}

fn invalid_command(stderr: &mut dyn Write) -> ExitCode {
    let _ = writeln!(
        stderr,
        "{{\"error\":{{\"code\":\"InvalidUsage\",\"message\":\"unknown command\"}}}}"
    );
    ExitCode::from(2)
}

#[cfg(windows)]
pub(super) fn refuse_configure_write(stderr: &mut dyn Write) -> ExitCode {
    super::errors::write_error(
        stderr,
        &super::errors::Envelope {
            code: "InvalidUsage",
            message: "configure writes are not supported on Windows yet. Edit the files by hand.",
            status: None,
            operation: None,
        },
    );
    ExitCode::from(2)
}

pub(super) fn parse(
    args: Vec<OsString>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    registry: &crate::services::Registry,
) -> Result<clap::ArgMatches, ExitCode> {
    match command_with_registry(registry).try_get_matches_from(args) {
        Ok(matches) => Ok(matches),
        Err(error) if !error.use_stderr() => {
            let _ = write!(stdout, "{}", error.render());
            Err(ExitCode::SUCCESS)
        }
        Err(error) => {
            let message = match error.kind() {
                clap::error::ErrorKind::InvalidSubcommand => "unknown command",
                clap::error::ErrorKind::UnknownArgument => "unknown argument or flag",
                clap::error::ErrorKind::InvalidValue => "invalid flag value",
                _ => "invalid command usage",
            };
            Err(ExitCode::from(super::dispatch::local(
                stderr,
                super::dispatch::LocalError::usage(message),
            )))
        }
    }
}

pub(super) fn run_local(
    matches: &clap::ArgMatches,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    registry: &crate::services::Registry,
) -> Option<ExitCode> {
    if let Some(help) = matches.subcommand_matches("help") {
        let path: Vec<&str> = help
            .get_many::<String>("path")
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        return Some(match render_help_path(registry, &path) {
            Some(text) => {
                let _ = write!(stdout, "{text}");
                ExitCode::SUCCESS
            }
            None => invalid_command(stderr),
        });
    }
    if matches.get_count("help-flag") > 0 {
        let mut path = Vec::new();
        let mut current = matches;
        while let Some((name, submatches)) = current.subcommand() {
            path.push(name);
            current = submatches;
        }
        return Some(match render_help_path(registry, &path) {
            Some(text) => {
                let _ = write!(stdout, "{text}");
                ExitCode::SUCCESS
            }
            None => invalid_command(stderr),
        });
    }
    #[cfg(windows)]
    if matches
        .subcommand_matches("configure")
        .is_some_and(|configure| matches!(configure.subcommand_name(), None | Some("set")))
    {
        return Some(refuse_configure_write(stderr));
    }
    if matches.get_flag("version-flag") || matches.subcommand_name() == Some("version") {
        let _ = writeln!(stdout, "{}", version());
    } else if let Some(docs) = matches.subcommand_matches("gen-docs") {
        if let Some(directory) = docs.get_one::<std::path::PathBuf>("directory")
            && super::gendocs::generate(directory, registry).is_err()
        {
            let _ = writeln!(
                stderr,
                "{{\"error\":{{\"code\":\"InvalidUsage\",\"message\":\"cannot generate documentation\"}}}}"
            );
            return Some(ExitCode::from(2));
        }
    } else if matches.subcommand_name() == Some("__command-inventory") {
        let _ = write!(stdout, "{}", inventory(registry));
    } else if let Some((name, submatches)) = matches.subcommand() {
        if submatches.subcommand().is_some() {
            return None;
        }
        if let Some(text) = render_help_path(registry, &[name]) {
            let _ = write!(stdout, "{text}");
        }
    } else {
        let _ = write!(stdout, "{}", command_with_registry(registry).render_help());
    }
    Some(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::{command, command_with_registry};
    use crate::cli::op::{Kind, Operation, Service};
    use crate::services::Registry;
    use std::io::Write;
    use std::process::ExitCode;

    fn run_with_registry(
        args: impl IntoIterator<Item = &'static str>,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
        registry: &Registry,
    ) -> ExitCode {
        match super::parse(
            args.into_iter().map(Into::into).collect(),
            stdout,
            stderr,
            registry,
        ) {
            Ok(matches) => super::run_local(&matches, stdout, stderr, registry).unwrap(),
            Err(exit) => exit,
        }
    }

    #[test]
    fn clustered_service_help_validates_all_arguments() {
        const OP: Operation = Operation {
            name: "inspect",
            sdk_operation: "Inspect",
            short_help: "Inspect",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: None,
        };
        let registry = Registry::new(&[Service {
            name: "toy",
            error_rules: &[],
            operations: &[OP],
        }])
        .unwrap();
        for args in [
            ["greennode", "toy", "-hh", "--nope"],
            ["greennode", "toy", "inspect", "-hh"],
        ] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run_with_registry(args, &mut out, &mut err, &registry);
            if args[2] == "-hh" {
                assert_eq!(code, std::process::ExitCode::from(2));
                assert!(out.is_empty());
            } else {
                assert_eq!(code, std::process::ExitCode::SUCCESS);
                assert!(!out.is_empty());
            }
        }
    }

    #[test]
    fn service_without_operation_prints_help() {
        const OP: Operation = Operation {
            name: "inspect",
            sdk_operation: "Inspect",
            short_help: "Inspect",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: None,
        };
        let registry = Registry::new(&[Service {
            name: "toy",
            error_rules: &[],
            operations: &[OP],
        }])
        .unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with_registry(["greennode", "toy"], &mut out, &mut err, &registry);
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert!(String::from_utf8(out).unwrap().contains("inspect"));
        assert!(err.is_empty());
    }

    #[test]
    fn registered_service_is_in_help_and_parser() {
        const OP: Operation = Operation {
            name: "inspect",
            sdk_operation: "Inspect",
            short_help: "Inspect",
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
            handler: None,
        };
        let registry = Registry::new(&[Service {
            name: "toy",
            error_rules: &[],
            operations: &[OP],
        }])
        .unwrap();
        assert!(super::inventory(&registry).contains("\"greennode toy\""));
        assert!(super::inventory(&registry).contains("\"greennode toy inspect\""));
        assert_ne!(
            super::inventory(&registry),
            include_str!("../../tests/parity/shipped-commands.json")
        );
        assert!(
            command_with_registry(&registry)
                .render_help()
                .to_string()
                .contains("toy")
        );
        let matches = command_with_registry(&registry)
            .try_get_matches_from(["greennode", "toy", "inspect"])
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("toy"));
        assert_eq!(
            matches.subcommand_matches("toy").unwrap().subcommand_name(),
            Some("inspect")
        );
    }

    #[test]
    fn boolean_values_are_preserved() {
        for value in ["true", "false"] {
            let arg = format!("--read-only={value}");
            for args in [
                vec!["greennode", arg.as_str(), "version"],
                vec!["greennode", "version", arg.as_str()],
            ] {
                let matches = command().try_get_matches_from(args).unwrap();
                assert_eq!(
                    matches.get_one::<bool>("read-only"),
                    Some(&(value == "true"))
                );
            }
        }
    }

    #[test]
    fn repeated_global_scalar_uses_last_value() {
        let matches = command()
            .try_get_matches_from(["greennode", "--profile=first", "--profile=last", "version"])
            .unwrap();
        assert_eq!(
            matches.get_one::<String>("profile").map(String::as_str),
            Some("last")
        );
        let matches = command()
            .try_get_matches_from(["greennode", "--profile=first", "version", "--profile=last"])
            .unwrap();
        assert_eq!(
            matches.get_one::<String>("profile").map(String::as_str),
            Some("last")
        );
    }
}
