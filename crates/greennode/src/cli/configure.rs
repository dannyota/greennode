mod set;

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode_sdk::{FileSlot, ProfileFile, Selection, read_slot, select};

use super::{Invocation, InvocationHandler, Terminal, dispatch};

const KEYS: [(&str, bool, bool); 11] = [
    ("region", false, false),
    ("project_id", false, false),
    ("output", false, false),
    ("read_only", false, false),
    ("root_email", true, false),
    ("username", true, false),
    ("password", true, true),
    ("totp_secret", true, true),
    ("vcdn_api_key", true, true),
    ("client_id", true, false),
    ("client_secret", true, true),
];

pub(super) struct Handler<'a>(pub(super) Option<&'a mut dyn Terminal>);

impl InvocationHandler for Handler<'_> {
    fn accepts(&self, args: &[OsString]) -> bool {
        let Ok(registry) = crate::services::registered() else {
            return false;
        };
        let Ok(matches) = super::root::command_with_registry(&registry).try_get_matches_from(args)
        else {
            return false;
        };
        if matches.get_count("help-flag") > 0 {
            return false;
        }
        matches.subcommand_matches("configure").is_some_and(|m| {
            matches!(m.subcommand_name(), Some("get" | "list" | "set"))
                || (cfg!(unix)
                    && m.subcommand_name().is_none()
                    && !matches.get_flag("version-flag"))
        })
    }

    fn run<'a>(
        &'a mut self,
        invocation: &'a mut Invocation<'_>,
    ) -> dispatch::FutureResult<'a, ExitCode> {
        Box::pin(async move { run(invocation, self.0.take()).await })
    }
}

fn escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                out.push_str(&format!("\\x{:02x}", u32::from(c)))
            }
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            _ => out.push(c),
        }
    }
    out
}

fn fail(invocation: &mut Invocation<'_>, code: &str, message: &str, exit: u8) -> ExitCode {
    super::errors::write_error(
        invocation.stderr,
        &super::errors::Envelope {
            code,
            message: message.into(),
            status: None,
            operation: None,
        },
    );
    ExitCode::from(exit)
}

fn value<'a>(file: &'a ProfileFile, section: &str, key: &str) -> &'a str {
    file.sections()
        .get(section)
        .and_then(|s| s.get(key))
        .map_or("", |v| v.expose_secret())
}

async fn run(invocation: &mut Invocation<'_>, _terminal: Option<&mut dyn Terminal>) -> ExitCode {
    let Ok(registry) = crate::services::registered() else {
        return ExitCode::FAILURE;
    };
    let Ok(matches) =
        super::root::command_with_registry(&registry).try_get_matches_from(&invocation.args)
    else {
        return ExitCode::FAILURE;
    };
    let Some(command) = matches.subcommand_matches("configure") else {
        return ExitCode::FAILURE;
    };
    let set_command = command.subcommand_matches("set");
    let key = set_command
        .or_else(|| command.subcommand_matches("get"))
        .and_then(|m| m.get_one::<String>("key"));
    let selected_key = key.and_then(|k| KEYS.iter().find(|(name, _, _)| *name == k));
    if key.is_some() && selected_key.is_none() {
        return fail(invocation, "InvalidUsage", "unknown configure key", 2);
    }
    let env_profile = ["GREENNODE_PROFILE", "VNGCLOUD_PROFILE"]
        .into_iter()
        .find_map(|name| {
            invocation
                .source
                .var_os(OsStr::new(name))
                .filter(|v| !v.is_empty())
        });
    let profile = match matches
        .get_one::<String>("profile")
        .filter(|v| !v.is_empty())
    {
        Some(profile) => profile.clone(),
        None => match env_profile.map(OsString::into_string).transpose() {
            Ok(Some(profile)) => profile,
            Ok(None) => "default".to_owned(),
            Err(_) => return fail(invocation, "InvalidConfig", "invalid profile name", 2),
        },
    };
    if profile.trim() != profile || profile.contains(['\r', '\n', '[', ']']) {
        return fail(
            invocation,
            "InvalidUsage",
            "profile name must not contain a newline, '[', ']', or leading or trailing whitespace",
            2,
        );
    }
    let cwd = std::env::current_dir().ok();
    let read_only = matches
        .get_one::<bool>("read-only")
        .copied()
        .unwrap_or(false);
    #[cfg(unix)]
    if command.subcommand_name().is_none() {
        return set::prompt(invocation, &profile, cwd.as_deref(), read_only, _terminal).await;
    }
    if let Some((set_command, &(name, credentials, secret))) = set_command.zip(selected_key) {
        let Some(argument) = set_command.get_one::<OsString>("value") else {
            return ExitCode::FAILURE;
        };
        return set::run(
            invocation,
            &profile,
            cwd.as_deref(),
            read_only,
            (name, credentials, secret),
            argument,
        )
        .await;
    }
    let selection = match select(None, None, invocation.source, cwd.as_deref()) {
        Ok(selection) => selection,
        Err(error) => return sdk_error(invocation, &error),
    };
    let need_config = selected_key.is_none_or(|(_, credentials, _)| !credentials);
    let need_credentials = selected_key.is_none_or(|(_, credentials, _)| *credentials);
    if (need_config && selection.config.is_none())
        || (need_credentials && selection.credentials.is_none())
    {
        return fail(
            invocation,
            "InvalidConfig",
            "configuration file path requires a home directory or override",
            2,
        );
    }
    let config = if need_config {
        match read_slot(&selection, FileSlot::Config, &profile) {
            Ok(file) => file,
            Err(error) => return sdk_error(invocation, &error),
        }
    } else {
        ProfileFile::default()
    };
    let credentials = if need_credentials {
        match read_slot(&selection, FileSlot::Credentials, &profile) {
            Ok(file) => file,
            Err(error) => return sdk_error(invocation, &error),
        }
    } else {
        ProfileFile::default()
    };
    let section = if profile == "default" {
        profile.clone()
    } else {
        format!("profile {profile}")
    };
    for (name, is_credentials, secret) in KEYS {
        if key.is_some_and(|k| k != name) {
            continue;
        }
        let (file, section) = if is_credentials {
            (&credentials, profile.as_str())
        } else {
            (&config, section.as_str())
        };
        let shown = if secret {
            if file.is_set(section, name) {
                "****"
            } else {
                ""
            }
            .to_owned()
        } else {
            escape(value(file, section, name))
        };
        let result = if key.is_some() {
            writeln!(invocation.stdout, "{shown}")
        } else {
            writeln!(invocation.stdout, "{name} = {shown}")
        };
        if result.is_err() {
            return ExitCode::FAILURE;
        }
    }
    if key.is_none()
        && value(&config, &section, "region").is_empty()
        && ["GREENNODE_REGION", "VNGCLOUD_REGION"].iter().all(|name| {
            invocation
                .source
                .var_os(OsStr::new(name))
                .is_none_or(|v| v.is_empty())
        })
        && hint(invocation, &selection).is_err()
    {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn sdk_error(invocation: &mut Invocation<'_>, error: &greennode_sdk::Error) -> ExitCode {
    let classified = super::errors::classify(error, &[]);
    let diagnostic = error
        .detail::<greennode_sdk::FileDiagnostic>()
        .map(|detail| {
            let slot = match detail.slot() {
                FileSlot::Config => "config",
                FileSlot::Credentials => "credentials",
            };
            let line = detail
                .line()
                .map_or_else(String::new, |line| format!(":{line}"));
            format!("{slot} ({}){line}: {}", detail.source(), detail.reason())
        });
    let envelope = super::errors::Envelope {
        message: diagnostic.map_or(classified.envelope.message, Into::into),
        ..classified.envelope
    };
    super::errors::write_error(invocation.stderr, &envelope);
    ExitCode::from(classified.exit)
}

fn hint(invocation: &mut Invocation<'_>, selection: &Selection) -> std::io::Result<()> {
    let (Some(config), Some(credentials)) = (&selection.config, &selection.credentials) else {
        return Ok(());
    };
    let prefix = if selection.mode == greennode_sdk::Mode::Legacy {
        "VNGCLOUD"
    } else {
        "GREENNODE"
    };
    writeln!(
        invocation.stderr,
        "hint: no region is set; read {} and {}. Set {prefix}_REGION or run: greennode configure set region <region>",
        escape(&config.to_string_lossy()),
        escape(&credentials.to_string_lossy())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::path::PathBuf;

    struct Source(PathBuf);
    impl greennode_sdk::ConfigSource for Source {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn list_fails_when_hint_cannot_be_written() {
        let home =
            std::env::temp_dir().join(format!("greennode-configure-hint-{}", std::process::id()));
        std::fs::create_dir(&home).unwrap();
        let source = Source(home.clone());
        let (context, _) = greennode_sdk::RequestContext::new();
        let mut input = io::empty();
        let mut output = Vec::new();
        let mut error = Failing;
        let mut invocation = Invocation {
            args: ["greennode", "configure", "list"]
                .map(OsString::from)
                .to_vec(),
            source: &source,
            config_options: greennode_sdk::ConfigOptions::default(),
            stdin: &mut input,
            stdout: &mut output,
            stderr: &mut error,
            stdin_terminal: false,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &greennode_sdk::SystemClock,
            context: &context,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(
            runtime.block_on(run(&mut invocation, None)),
            ExitCode::FAILURE
        );
        std::fs::remove_dir(home).unwrap();
    }
}
