use super::{Stdin, dispatch::LocalError, input::InputError};
use clap::ArgMatches;
use greennode_sdk::{Clock, RequestContext, Secret};
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;
use zeroize::Zeroizing;

pub(crate) const HELP: &str = "Private file holding the master password, or - for stdin";
pub(crate) const FIELD: super::op::Field = super::op::Field {
    name: "password-file",
    go_type: "string",
    required: false,
};

pub(crate) fn path(matches: &ArgMatches) -> Option<&Path> {
    matches
        .get_one::<OsString>(FIELD.name)
        .filter(|path| !path.is_empty())
        .map(Path::new)
}

pub(crate) fn check_conflict(path: &Path, json: Option<&str>) -> Result<(), InputError> {
    if path == Path::new("-") && json.is_some_and(|json| json.starts_with("file://")) {
        return Err(InputError(
            "--password-file - cannot share input with file:// JSON",
        ));
    }
    Ok(())
}

pub(crate) async fn read(
    ctx: &RequestContext,
    clock: &dyn Clock,
    path: &Path,
    stdin: &mut dyn Stdin,
    stdin_terminal: bool,
) -> Result<Secret, LocalError> {
    read_input(ctx, clock, path, stdin, stdin_terminal, FileInput::Password).await
}

#[derive(Clone, Copy)]
pub(crate) enum FileInput {
    Password,
    Certificate {
        flag: &'static str,
        private: bool,
        trim_newline: bool,
    },
}

impl FileInput {
    fn flag(self) -> &'static str {
        match self {
            Self::Password => FIELD.name,
            Self::Certificate { flag, .. } => flag,
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Password => 4096,
            Self::Certificate { .. } => 64 * 1024,
        }
    }

    fn error(self, message: &str) -> LocalError {
        LocalError::usage(format!("--{}: {message}", self.flag()))
    }
}

pub(crate) async fn read_input(
    ctx: &RequestContext,
    clock: &dyn Clock,
    path: &Path,
    stdin: &mut dyn Stdin,
    stdin_terminal: bool,
    input: FileInput,
) -> Result<Secret, LocalError> {
    let cancelled = || input.error("input canceled or timed out");
    if ctx.check(clock).is_some() {
        return Err(cancelled());
    }
    if path != Path::new("-") {
        let path: PathBuf = path.to_owned();
        let result = tokio::task::spawn_blocking(move || read_file(&path, input))
            .await
            .map_err(|_| input.error("input could not be read"))?;
        if ctx.check(clock).is_some() {
            return Err(cancelled());
        }
        return result;
    }
    if stdin_terminal {
        return Err(LocalError::usage(format!(
            "--{} - refuses a terminal",
            input.flag()
        )));
    }
    let deadline = clock.monotonic_now() + Duration::from_secs(30);
    let ctx = ctx.child_with_deadline(deadline);
    let deadline = ctx.deadline().unwrap_or(deadline);
    let result = read_bytes(input, |bytes| {
        if ctx.check(clock).is_some() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stdin.read_before(bytes, deadline)
    });
    if ctx.check(clock).is_some() {
        return Err(cancelled());
    }
    result
}

fn read_file(path: &Path, input: FileInput) -> Result<Secret, LocalError> {
    let mut file = open(path).map_err(|_| input.error("cannot open input"))?;
    let metadata = file
        .metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .ok_or_else(|| input.error("input must be a regular file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = matches!(
            input,
            FileInput::Password | FileInput::Certificate { private: true, .. }
        );
        if private && metadata.permissions().mode() & 0o077 != 0 {
            return Err(input.error("input must be private (mode 0600)"));
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    read_bytes(input, |bytes| file.read(bytes))
}

#[cfg(unix)]
fn open(path: &Path) -> io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, open};
    Ok(open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into())
}

#[cfg(not(unix))]
fn open(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn read_bytes(
    input: FileInput,
    mut read: impl FnMut(&mut [u8]) -> io::Result<usize>,
) -> Result<Secret, LocalError> {
    let mut data = Zeroizing::new(vec![0; input.limit() + 1]);
    let mut offset = 0;
    loop {
        let remaining = data
            .get_mut(offset..)
            .filter(|remaining| !remaining.is_empty())
            .ok_or_else(|| input.error(&format!("input exceeds {} bytes", input.limit())))?;
        match read(remaining) {
            Ok(0) => break,
            Ok(count) if count <= remaining.len() => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(input.error("input canceled or timed out"));
            }
            _ => return Err(input.error("input could not be read")),
        }
    }
    let bytes = data.get(..offset).unwrap_or_default();
    let text = std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| input.error("input must be non-empty valid UTF-8"))?;
    let text = if matches!(
        input,
        FileInput::Password
            | FileInput::Certificate {
                trim_newline: true,
                ..
            }
    ) {
        text.strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text)
    } else {
        text
    };
    if matches!(input, FileInput::Password) && text.is_empty() {
        return Err(input.error("input must be non-empty valid UTF-8"));
    }
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let newline = ch == '\n' || (ch == '\r' && chars.peek() == Some(&'\n'));
        if ch.is_control() && (matches!(input, FileInput::Password) || !newline) {
            return Err(input.error("input must not contain controls"));
        }
    }
    Ok(Secret::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use greennode_sdk::TestClock;
    use tokio::time::Instant;

    #[test]
    fn certificate_mode_preserves_newlines_and_password_mode_remains_single_line() {
        let mode = FileInput::Certificate {
            flag: "private-key-file",
            private: true,
            trim_newline: false,
        };
        let input = b"first\r\nsecond\n";
        let read = |mode, mut bytes: &[u8]| read_bytes(mode, |out| bytes.read(out));
        assert!(read(mode, input).is_ok_and(|value| value.expose_secret().as_bytes() == input));
        assert!(read(mode, b"line\rrest").is_err());
        assert!(read(FileInput::Password, input).is_err());
        assert!(
            read(FileInput::Password, b"toy\r\n").is_ok_and(|value| value.expose_secret() == "toy")
        );
        let long = vec![b'a'; 65536];
        assert!(read(mode, &long).is_ok());
        assert!(read(mode, &[long.as_slice(), b"a"].concat()).is_err());
        assert!(read(FileInput::Password, &long).is_err());
    }

    struct TimedInput<'a> {
        clock: &'a TestClock,
        observed: Option<Instant>,
    }

    impl Read for TimedInput<'_> {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("password stdin must use the timed read path");
        }
    }

    impl Stdin for TimedInput<'_> {
        fn read_before(&mut self, _: &mut [u8], deadline: Instant) -> io::Result<usize> {
            self.observed = Some(deadline);
            self.clock.advance(deadline - self.clock.monotonic_now());
            Err(io::ErrorKind::TimedOut.into())
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn injected_blocking_pipe_cannot_hold_password_input_past_its_deadline() {
        use crate::cli::BlockingStdin;
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        let (mut reader, writer) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let mut borrowed: &mut dyn Read = &mut reader;
        let clock = greennode_sdk::SystemClock;
        let (context, _) = RequestContext::new();
        let error = read(&context, &clock, Path::new("-"), &mut borrowed, false)
            .await
            .unwrap_err();
        assert_eq!(
            error.message,
            "--password-file: input canceled or timed out"
        );

        reader.set_read_timeout(None).unwrap();
        let mut input = BlockingStdin::new(reader);
        let deadline = clock.monotonic_now() + Duration::from_millis(25);
        let context = context.child_with_deadline(deadline);
        let result = read(&context, &clock, Path::new("-"), &mut input, false).await;
        drop(writer);
        assert!(clock.monotonic_now() >= deadline);
        assert_eq!(
            result.unwrap_err().message,
            "--password-file: input canceled or timed out"
        );

        let (reader, mut writer) = UnixStream::pair().unwrap();
        let toy_password = "Synthetic_Password32";
        assert!(writer.write_all(toy_password.as_bytes()).is_ok());
        drop(writer);
        let (context, _) = RequestContext::new();
        let result = read(
            &context,
            &clock,
            Path::new("-"),
            &mut BlockingStdin::new(reader),
            false,
        )
        .await;
        assert!(result.is_ok_and(|secret| secret.expose_secret() == toy_password));
    }

    #[tokio::test]
    async fn stdin_supplies_the_shorter_of_its_timeout_and_the_caller_deadline() {
        let clock = TestClock::new(greennode_sdk::SystemClock.utc_now());
        let (context, _) = RequestContext::new();
        for (limit, expected) in [(45, 30), (5, 5)] {
            let start = clock.monotonic_now();
            let context = context.child_with_deadline(start + Duration::from_secs(limit));
            let mut input = TimedInput {
                clock: &clock,
                observed: None,
            };
            let error = read(&context, &clock, Path::new("-"), &mut input, false)
                .await
                .unwrap_err();
            assert_eq!(input.observed, Some(start + Duration::from_secs(expected)));
            assert_eq!(
                error.message,
                "--password-file: input canceled or timed out"
            );
        }
    }
}
