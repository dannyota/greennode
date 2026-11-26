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
            read_answer(invocation, true)?
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

fn read_answer(invocation: &mut Invocation<'_>, allow_eof: bool) -> Result<Zeroizing<String>> {
    let mut input = Zeroizing::new([0; STDIN_LIMIT + 2]);
    let mut length = 0;
    while let Some(byte) = input.get_mut(length) {
        check(invocation)?;
        let result = invocation.stdin.read(std::slice::from_mut(byte));
        check(invocation)?;
        match result {
            Ok(0) if allow_eof => break,
            Ok(1) if *byte == b'\n' => break,
            Ok(1) => length += 1,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            _ => return Err(Error::Terminal("reading configure input failed")),
        }
    }
    let answer = &input[..length];
    let answer = answer.strip_suffix(b"\r").unwrap_or(answer);
    if answer.len() > STDIN_LIMIT {
        return Err(Error::Usage(
            "stdin value is larger than 4096 bytes".to_owned(),
        ));
    }
    std::str::from_utf8(answer)
        .map(|value| Zeroizing::new(value.to_owned()))
        .map_err(|_| Error::Usage("stdin value is not valid UTF-8".to_owned()))
}

fn read_secret(
    invocation: &mut Invocation<'_>,
    terminal: &mut dyn Terminal,
    label: &str,
    shown: &str,
) -> Result<Zeroizing<String>> {
    write_prompt(invocation.stdout, label, shown)?;
    check(invocation)?;
    let echo = Echo::disable(terminal)?;
    let answer = read_answer(invocation, false);
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

#[cfg(test)]
mod tests {
    use super::*;
    use greennode_sdk::{ConfigSource, RequestContext, SystemClock};
    use std::cell::Cell;
    use std::ffi::{OsStr, OsString};
    use std::io::Read;

    struct EmptySource;

    impl ConfigSource for EmptySource {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
    }

    fn invocation<'a>(
        stdin: &'a mut dyn Read,
        stdout: &'a mut dyn Write,
        stderr: &'a mut dyn Write,
        context: &'a RequestContext,
    ) -> Invocation<'a> {
        Invocation {
            args: Vec::new(),
            source: &EmptySource,
            config_options: greennode_sdk::ConfigOptions::default(),
            stdin,
            stdout,
            stderr,
            stdin_terminal: true,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context,
        }
    }

    struct FakeTerminal<'a>(&'a Cell<bool>);

    impl Terminal for FakeTerminal<'_> {
        fn disable_echo(&mut self) -> io::Result<()> {
            self.0.set(false);
            Ok(())
        }

        fn restore_echo(&mut self, _: bool) -> io::Result<()> {
            self.0.set(true);
            Ok(())
        }
    }

    struct CancellingOutput<'a, F> {
        echo: &'a Cell<bool>,
        cancel: F,
    }

    impl<F: FnMut()> Write for CancellingOutput<'_, F> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(self.echo.get(), "writing the prompt with echo disabled");
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            (self.cancel)();
            assert!(self.echo.get(), "flushing the prompt with echo disabled");
            Ok(())
        }
    }

    #[test]
    fn cancellation_while_writing_a_secret_prompt_leaves_echo_enabled() {
        let (context, cancel) = RequestContext::new();
        let echo = Cell::new(true);
        let mut terminal = FakeTerminal(&echo);
        let mut input = b"toy-password\n".as_slice();
        let mut output = CancellingOutput {
            echo: &echo,
            cancel: || cancel.cancel(),
        };
        let mut error = io::sink();
        let mut invocation = invocation(&mut input, &mut output, &mut error, &context);

        assert!(matches!(
            read_secret(&mut invocation, &mut terminal, "Password", ""),
            Err(Error::Sdk(_))
        ));
        assert!(echo.get());
        assert_eq!(input.len(), b"toy-password\n".len());
    }

    struct EofThenInput<'a> {
        eof: bool,
        next: &'a [u8],
    }

    impl Read for EofThenInput<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.eof) {
                return Ok(0);
            }
            self.next.read(output)
        }
    }

    #[test]
    fn visible_eof_is_blank_and_the_next_answer_is_read() {
        let (context, _) = RequestContext::new();
        let mut input = EofThenInput {
            eof: true,
            next: b"toy-project\n",
        };
        let mut output = io::sink();
        let mut error = io::sink();
        let mut invocation = invocation(&mut input, &mut output, &mut error, &context);

        assert!(matches!(read_answer(&mut invocation, true), Ok(answer) if answer.is_empty()));
        assert!(
            matches!(read_answer(&mut invocation, true), Ok(answer) if *answer == "toy-project")
        );
    }

    #[test]
    fn empty_secret_eof_fails_and_restores_echo() {
        let (context, _) = RequestContext::new();
        let echo = Cell::new(true);
        let mut terminal = FakeTerminal(&echo);
        let mut input = io::empty();
        let mut output = io::sink();
        let mut error = io::sink();
        let mut invocation = invocation(&mut input, &mut output, &mut error, &context);

        assert!(matches!(
            read_secret(&mut invocation, &mut terminal, "Password", ""),
            Err(Error::Terminal("reading configure input failed"))
        ));
        assert!(echo.get());
    }
}
