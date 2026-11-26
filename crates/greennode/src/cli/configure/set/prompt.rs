use std::io::{self, Write};
use std::path::Path;

use greennode_sdk::{FileSlot, read_slot};
use zeroize::Zeroizing;

use super::super::{KEYS, escape, value};
use super::{Error, Invocation, Result, STDIN_LIMIT, Snapshot, check, publish, validate};
use crate::cli::Terminal;

const LABELS: [&str; 8] = [
    "Region",
    "Project ID",
    "Output format",
    "Read only",
    "Root email",
    "Username",
    "Password",
    "TOTP secret",
];

pub(super) async fn execute(
    invocation: &mut Invocation<'_>,
    profile: &str,
    cwd: Option<&Path>,
    flag: bool,
    terminal: Option<&mut dyn Terminal>,
) -> Result<()> {
    check(invocation)?;
    let (snapshot, config) = Snapshot::read(invocation.source, cwd, profile, flag)?;
    let Some(terminal) = terminal.filter(|_| invocation.stdin_terminal) else {
        return Err(Error::Usage(
            "configure requires a terminal on stdin; use configure set <key> -".to_owned(),
        ));
    };
    let credentials = read_slot(&snapshot.selection, FileSlot::Credentials, profile)?;
    let config_section = if profile == "default" {
        profile.to_owned()
    } else {
        format!("profile {profile}")
    };
    let mut config_changes = Vec::new();
    let mut credential_changes = Vec::new();
    for ((key, is_credential, secret), label) in KEYS.into_iter().zip(LABELS) {
        check(invocation)?;
        let (file, section, changes) = if is_credential {
            (&credentials, profile, &mut credential_changes)
        } else {
            (&config, config_section.as_str(), &mut config_changes)
        };
        let answer = if secret {
            let shown = if file.is_set(section, key) {
                "****"
            } else {
                ""
            };
            read_secret(invocation, terminal, label, shown)?
        } else {
            write_prompt(invocation.stdout, label, &escape(value(file, section, key)))?;
            read_answer(invocation)?
        };
        if !answer.is_empty() {
            validate(key, &answer)?;
            changes.push((key, answer));
        }
    }
    check(invocation)?;
    if config_changes.is_empty() && credential_changes.is_empty() {
        return Ok(());
    }
    let changes = [&config_changes, &credential_changes].map(|values| {
        values
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>()
    });
    publish(
        invocation,
        profile,
        cwd,
        flag,
        &snapshot,
        changes.each_ref().map(Vec::as_slice),
    )
    .await
}

fn write_prompt(output: &mut dyn Write, label: &str, shown: &str) -> Result<()> {
    write!(output, "{label} [{shown}]: ")
        .and_then(|()| output.flush())
        .map_err(|_| Error::Terminal("writing configure prompt failed"))
}

fn read_answer(invocation: &mut Invocation<'_>) -> Result<Zeroizing<String>> {
    let mut input = Zeroizing::new([0; STDIN_LIMIT + 2]);
    let mut length = 0;
    while let Some(byte) = input.get_mut(length) {
        check(invocation)?;
        let result = invocation.stdin.read(std::slice::from_mut(byte));
        check(invocation)?;
        match result {
            Ok(1) if *byte == b'\n' => {
                let answer = &input[..length];
                let answer = answer.strip_suffix(b"\r").unwrap_or(answer);
                if answer.len() > STDIN_LIMIT {
                    break;
                }
                return std::str::from_utf8(answer)
                    .map(|value| Zeroizing::new(value.to_owned()))
                    .map_err(|_| Error::Usage("stdin value is not valid UTF-8".to_owned()));
            }
            Ok(1) => length += 1,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            _ => return Err(Error::Terminal("reading configure input failed")),
        }
    }
    Err(Error::Usage(
        "stdin value is larger than 4096 bytes".to_owned(),
    ))
}

fn read_secret(
    invocation: &mut Invocation<'_>,
    terminal: &mut dyn Terminal,
    label: &str,
    shown: &str,
) -> Result<Zeroizing<String>> {
    let echo = Echo::disable(terminal)?;
    write_prompt(invocation.stdout, label, shown)?;
    let answer = read_answer(invocation);
    let restored = echo.restore(answer.is_err());
    let newline = writeln!(invocation.stdout).and_then(|()| invocation.stdout.flush());
    restored?;
    let answer = answer?;
    newline.map_err(|_| Error::Terminal("writing configure prompt failed"))?;
    Ok(answer)
}

struct Echo<'a> {
    terminal: &'a mut dyn Terminal,
    restored: bool,
}

impl<'a> Echo<'a> {
    fn disable(terminal: &'a mut dyn Terminal) -> Result<Self> {
        let echo = Self {
            terminal,
            restored: false,
        };
        echo.terminal
            .disable_echo()
            .map_err(|_| Error::Terminal("cannot disable terminal echo"))?;
        Ok(echo)
    }

    fn restore(mut self, discard_input: bool) -> Result<()> {
        self.terminal
            .restore_echo(discard_input)
            .map_err(|_| Error::Terminal("cannot restore terminal echo"))?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for Echo<'_> {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.terminal.restore_echo(true);
        }
    }
}
