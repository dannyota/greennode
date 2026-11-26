use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use greennode_sdk::{FileSlot, ProfileFile, Selection, read_slot, select};

use super::{Invocation, InvocationHandler, dispatch};

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

pub(super) struct Handler;

impl InvocationHandler for Handler {
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
        matches
            .subcommand_matches("configure")
            .is_some_and(|m| matches!(m.subcommand_name(), Some("get" | "list")))
    }

    fn run<'a>(
        &'a mut self,
        invocation: &'a mut Invocation<'_>,
    ) -> dispatch::FutureResult<'a, ExitCode> {
        Box::pin(async move { run(invocation) })
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
            message,
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

fn run(invocation: &mut Invocation<'_>) -> ExitCode {
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
    let key = command
        .subcommand_matches("get")
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
        return fail(invocation, "InvalidConfig", "invalid profile name", 2);
    }
    let cwd = std::env::current_dir().ok();
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
        match read_slot(&selection, FileSlot::Config) {
            Ok(file) => file,
            Err(error) => return sdk_error(invocation, &error),
        }
    } else {
        ProfileFile::default()
    };
    let credentials = if need_credentials {
        match read_slot(&selection, FileSlot::Credentials) {
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
    {
        hint(invocation, &selection);
    }
    ExitCode::SUCCESS
}

fn sdk_error(invocation: &mut Invocation<'_>, error: &greennode_sdk::Error) -> ExitCode {
    let classified = super::errors::classify(error, &[]);
    super::errors::write_error(invocation.stderr, &classified.envelope);
    ExitCode::from(classified.exit)
}

fn hint(invocation: &mut Invocation<'_>, selection: &Selection) {
    let (Some(config), Some(credentials)) = (&selection.config, &selection.credentials) else {
        return;
    };
    let prefix = if selection.mode == greennode_sdk::Mode::Legacy {
        "VNGCLOUD"
    } else {
        "GREENNODE"
    };
    let _ = writeln!(
        invocation.stderr,
        "hint: no region is set; read {} and {}. Set {prefix}_REGION or run: greennode configure set region <region>",
        escape(&config.to_string_lossy()),
        escape(&credentials.to_string_lossy())
    );
}
