use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::{Arg, ArgAction, Command};

fn command() -> Command {
    let mut root = Command::new("greennode")
        .about("Command-line access to GreenNode")
        .disable_version_flag(true)
        .args_override_self(true)
        .arg(
            Arg::new("version-flag")
                .long("version")
                .short('v')
                .action(ArgAction::SetTrue),
        )
        .subcommand(Command::new("version").about("Print the greennode version"))
        .subcommand(Command::new("__command-inventory").hide(true));
    for &name in crate::services::GLOBAL_FLAGS {
        let action = if matches!(name, "yes" | "read-only" | "debug") {
            ArgAction::SetTrue
        } else {
            ArgAction::Set
        };
        root = root.arg(Arg::new(name).long(name).global(true).action(action));
    }
    root
}

fn command_with_registry(registry: &crate::services::Registry) -> Command {
    let mut root = command();
    for service in registry.services() {
        let mut service_command = Command::new(service.name);
        for operation in service.operations {
            let mut operation_command = Command::new(operation.name);
            for flag in operation.flags.iter().chain(operation.extra_flags) {
                operation_command = operation_command.arg(Arg::new(flag.name).long(flag.name));
            }
            service_command = service_command.subcommand(operation_command);
        }
        root = root.subcommand(service_command);
    }
    root
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
    for service in registry.services() {
        for operation in service.operations {
            paths.push(format!("greennode {} {}", service.name, operation.name));
        }
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

pub fn run<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let registry = match crate::services::registered() {
        Ok(registry) => registry,
        Err(error) => {
            let _ = writeln!(stderr, "invalid command registry: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    run_with_registry(args, stdout, stderr, &registry)
}

fn run_with_registry<I, T>(
    args: I,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    registry: &crate::services::Registry,
) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = match command_with_registry(registry).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let rendered = error.render();
            let _ = if error.use_stderr() {
                write!(stderr, "{rendered}")
            } else {
                write!(stdout, "{rendered}")
            };
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
        }
    };
    if matches.get_flag("version-flag") || matches.subcommand_name() == Some("version") {
        let _ = writeln!(stdout, "{}", version());
    } else if matches.subcommand_name() == Some("__command-inventory") {
        let _ = write!(stdout, "{}", inventory(registry));
    } else if let Some((name, submatches)) = matches.subcommand() {
        if submatches.subcommand().is_none()
            && let Some(service) = command_with_registry(registry).find_subcommand_mut(name)
        {
            let _ = write!(stdout, "{}", service.render_help());
        }
    } else {
        let _ = write!(stdout, "{}", command_with_registry(registry).render_help());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{command, command_with_registry};
    use crate::cli::op::{Kind, Operation, Service};
    use crate::services::Registry;

    #[test]
    fn service_without_operation_prints_help() {
        const OP: Operation = Operation {
            name: "inspect",
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
        };
        let registry = Registry::new(&[Service {
            name: "toy",
            operations: &[OP],
        }])
        .unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = super::run_with_registry(["greennode", "toy"], &mut out, &mut err, &registry);
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert!(String::from_utf8(out).unwrap().contains("inspect"));
        assert!(err.is_empty());
    }

    #[test]
    fn registered_service_is_in_help_and_parser() {
        const OP: Operation = Operation {
            name: "inspect",
            kind: Kind::Read,
            flags: &[],
            json_fields: &[],
            extra_flags: &[],
            notes: &[],
        };
        let registry = Registry::new(&[Service {
            name: "toy",
            operations: &[OP],
        }])
        .unwrap();
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
