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
    let cancelled = || LocalError::usage("--password-file: input canceled or timed out");
    if ctx.check(clock).is_some() {
        return Err(cancelled());
    }
    if path != Path::new("-") {
        let path: PathBuf = path.to_owned();
        let result = tokio::task::spawn_blocking(move || read_file(&path))
            .await
            .map_err(|_| LocalError::usage("--password-file: input could not be read"))?;
        if ctx.check(clock).is_some() {
            return Err(cancelled());
        }
        return result;
    }
    if stdin_terminal {
        return Err(LocalError::usage("--password-file - refuses a terminal"));
    }
    let deadline = clock.monotonic_now() + Duration::from_secs(30);
    let ctx = ctx.child_with_deadline(deadline);
    let deadline = ctx.deadline().unwrap_or(deadline);
    let result = read_bytes(|bytes| {
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

fn read_file(path: &Path) -> Result<Secret, LocalError> {
    let file = open(path).map_err(|_| LocalError::usage("--password-file: cannot open input"))?;
    let metadata = file
        .metadata()
        .ok()
        .filter(std::fs::Metadata::is_file)
        .ok_or_else(|| LocalError::usage("--password-file: input must be a regular file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(LocalError::usage(
                "--password-file: input must be private (mode 0600)",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    let mut file = file;
    read_bytes(|bytes| file.read(bytes))
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

fn read_bytes(mut read: impl FnMut(&mut [u8]) -> io::Result<usize>) -> Result<Secret, LocalError> {
    let mut data = Zeroizing::new([0; 4097]);
    let mut offset = 0;
    loop {
        let remaining = data
            .get_mut(offset..)
            .filter(|remaining| !remaining.is_empty())
            .ok_or_else(|| LocalError::usage("--password-file: input exceeds 4096 bytes"))?;
        match read(remaining) {
            Ok(0) => break,
            Ok(count) if count <= remaining.len() => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(LocalError::usage(
                    "--password-file: input canceled or timed out",
                ));
            }
            _ => {
                return Err(LocalError::usage(
                    "--password-file: input could not be read",
                ));
            }
        }
    }
    let bytes = data.get(..offset).unwrap_or_default();
    let text = std::str::from_utf8(bytes)
        .ok()
        .map(|text| {
            text.strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(text)
        })
        .filter(|text| !text.is_empty())
        .ok_or_else(|| LocalError::usage("--password-file: input must be non-empty valid UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(LocalError::usage(
            "--password-file: input must not contain controls",
        ));
    }
    Ok(Secret::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use greennode_sdk::TestClock;
    use tokio::time::Instant;

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
