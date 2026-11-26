mod files;
mod ini;

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use greennode_sdk::{ConfigSource, FileSlot, Mode, Selection, read_slot, select};
use zeroize::Zeroizing;

use super::{Invocation, fail, sdk_error};

const STDIN_LIMIT: usize = 4096;

#[cfg(unix)]
const LEGACY_RECIPE: &str = r#"Stay in the same working directory and check the sources with greennode configure list.
Verify that the real destination is outside the legacy tree. Use your private
directory and stop other configuration editors during the copy.

macOS and Linux:

(
  set -eu
  umask 077
  d="$HOME/.greennode"
  mkdir -p "$d"
  test ! -L "$d"
  for f in config credentials; do
    test ! -e "$d/$f" && test ! -L "$d/$f" || exit 1
  done
  t=$(mktemp -d "$d/.migration.XXXXXX")
  trap 'rm -f "$t/config" "$t/credentials"; rmdir "$t"' 0
  cat < "${VNGCLOUD_CONFIG_FILE:-$HOME/.vngcloud/config}" > "$t/config"
  cat < "${VNGCLOUD_SHARED_CREDENTIALS_FILE:-$HOME/.vngcloud/credentials}" \
    > "$t/credentials"
  link "$t/config" "$d/config"
  link "$t/credentials" "$d/credentials"
)

If a legacy default is absent and has no override, use : > "$t/config" or
: > "$t/credentials" in place of that copy line. Keep unreadable sources and
missing overrides as errors. If only one link succeeds, keep both legacy
sources and complete or undo the copy before running commands. Set HOME first
if needed. Verify the copied settings and read_only with greennode configure list."#;

enum Error {
    Sdk(Box<greennode_sdk::Error>),
    Legacy,
    Config(&'static str),
    ReadOnly(String),
    Usage(String),
    File(TargetSlot, String),
}

impl From<greennode_sdk::Error> for Error {
    fn from(error: greennode_sdk::Error) -> Self {
        Self::Sdk(Box::new(error))
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Eq, PartialEq)]
struct TargetSlot {
    slot: FileSlot,
    named: bool,
}

impl TargetSlot {
    fn error(self, reason: &str, line: Option<usize>) -> Error {
        let (slot, source) = match (self.slot, self.named) {
            (FileSlot::Config, true) => ("config", "GREENNODE_CONFIG_FILE"),
            (FileSlot::Credentials, true) => ("credentials", "GREENNODE_SHARED_CREDENTIALS_FILE"),
            (FileSlot::Config, false) => ("config", "~/.greennode"),
            (FileSlot::Credentials, false) => ("credentials", "~/.greennode"),
        };
        let location = line.map_or_else(String::new, |number| format!(" line {number}"));
        Error::File(self, format!("{slot} ({source}){location}: {reason}"))
    }
}

pub(super) async fn run(
    invocation: &mut Invocation<'_>,
    profile: &str,
    cwd: Option<&Path>,
    flag: bool,
    key: (&str, bool, bool),
    argument: &OsStr,
) -> ExitCode {
    match execute(invocation, profile, cwd, flag, key, argument).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Sdk(error)) => {
            if let Some(diagnostic) = error.detail::<greennode_sdk::FileDiagnostic>() {
                let classified = super::super::errors::classify(&error, &[]);
                let slot = match diagnostic.slot() {
                    FileSlot::Config => "config",
                    FileSlot::Credentials => "credentials",
                };
                let location = diagnostic
                    .line()
                    .map_or_else(String::new, |number| format!(" line {number}"));
                let message = format!(
                    "{slot} ({}){location}: {}",
                    diagnostic.source(),
                    diagnostic.reason()
                );
                fail(
                    invocation,
                    classified.envelope.code,
                    &message,
                    classified.exit,
                )
            } else {
                sdk_error(invocation, &error)
            }
        }
        Err(Error::Legacy) => {
            let exit = fail(
                invocation,
                "InvalidConfig",
                "legacy configuration is read-only; no files changed",
                2,
            );
            #[cfg(unix)]
            let _ = writeln!(invocation.stderr, "{LEGACY_RECIPE}");
            #[cfg(not(unix))]
            let _ = writeln!(
                invocation.stderr,
                "Stay in the same working directory and check the sources with greennode configure list. \
                 In your private directory, stage copies of VNGCLOUD_CONFIG_FILE and VNGCLOUD_SHARED_CREDENTIALS_FILE \
                 (defaults: ~/.vngcloud/config and ~/.vngcloud/credentials). Verify that ~/.greennode resolves \
                 outside the legacy tree. Publish the staged files to ~/.greennode/config and ~/.greennode/credentials \
                 without overwriting either file. Stop other configuration editors during the copy. \
                 Verify the copied settings and read_only with greennode configure list."
            );
            exit
        }
        Err(Error::Config(message)) => fail(invocation, "InvalidConfig", message, 2),
        Err(Error::ReadOnly(source)) => {
            let message = format!("read-only: refused by {source}");
            fail(invocation, "ReadOnly", &message, 2)
        }
        Err(Error::Usage(message)) => fail(invocation, "InvalidUsage", &message, 2),
        Err(Error::File(slot, message)) if slot.slot == FileSlot::Credentials => {
            fail(invocation, "NoCredentials", &message, 3)
        }
        Err(Error::File(_, message)) => fail(invocation, "InvalidConfig", &message, 2),
    }
}

fn latch(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" => Some(false),
        "1" | "true" => Some(true),
        _ => None,
    }
}

fn environment_latch(source: &dyn ConfigSource, flag: bool) -> Result<()> {
    let mut enabled = flag.then_some("--read-only");
    for name in ["GREENNODE_READ_ONLY", "VNGCLOUD_READ_ONLY"] {
        let value = source.var_os(OsStr::new(name)).unwrap_or_default();
        match value.to_str().and_then(latch) {
            Some(true) if enabled.is_none() => enabled = Some(name),
            Some(_) => {}
            None => {
                return Err(Error::Config(
                    "read-only environment must be 1, true, 0, false, or empty",
                ));
            }
        }
    }
    enabled.map_or(Ok(()), |name| Err(Error::ReadOnly(name.to_owned())))
}

struct Snapshot {
    paths: [PathBuf; 2],
    slots: [TargetSlot; 2],
    legacy: files::LegacyTree,
    present: [bool; 2],
}

impl Snapshot {
    fn read(
        source: &dyn ConfigSource,
        cwd: Option<&Path>,
        profile: &str,
        flag: bool,
    ) -> Result<Self> {
        let selection = select(None, None, source, cwd)?;
        if selection.mode == Mode::Legacy {
            return Err(Error::Legacy);
        }
        environment_latch(source, flag)?;
        let home = source
            .home_dir()
            .filter(|home| home.is_absolute())
            .ok_or(Error::Config("configure set requires a home directory"))?;
        let legacy = files::LegacyTree::new(&home)?;
        let slots = [
            TargetSlot {
                slot: FileSlot::Config,
                named: selection.config_named,
            },
            TargetSlot {
                slot: FileSlot::Credentials,
                named: selection.credentials_named,
            },
        ];
        let paths = [
            resolve_target(&selection, &legacy, slots[0], selection.config.as_deref())?,
            resolve_target(
                &selection,
                &legacy,
                slots[1],
                selection.credentials.as_deref(),
            )?,
        ];
        let config = read_slot(&selection, FileSlot::Config, profile)?;
        if config.read_only(profile).map_err(|_| {
            Error::Config("read_only in the selected profile must be 1, true, 0, false, or empty")
        })? {
            return Err(Error::ReadOnly(format!(
                "read_only in profile {:?}",
                super::escape(profile)
            )));
        }
        Ok(Self {
            paths,
            slots,
            legacy,
            present: [
                selection.present(FileSlot::Config),
                selection.present(FileSlot::Credentials),
            ],
        })
    }

    fn recheck(
        &self,
        invocation: &Invocation<'_>,
        cwd: Option<&Path>,
        profile: &str,
        flag: bool,
    ) -> Result<Self> {
        check(invocation)?;
        let latest =
            Self::read(invocation.source, cwd, profile, flag).map_err(|error| match error {
                Error::Legacy => Error::Config("configuration mode changed during configure set"),
                other => other,
            })?;
        if self.paths != latest.paths || self.legacy != latest.legacy || self.slots != latest.slots
        {
            return Err(Error::Config(
                "configuration paths changed during configure set",
            ));
        }
        check(invocation)?;
        Ok(latest)
    }
}

fn resolve_target(
    selection: &Selection,
    legacy: &files::LegacyTree,
    slot: TargetSlot,
    path: Option<&Path>,
) -> Result<PathBuf> {
    let path = path.ok_or(Error::Config("missing configuration file path"))?;
    let real = files::resolve(path, selection.present(slot.slot), slot)?;
    legacy.check(
        real.parent()
            .ok_or(Error::Config("configuration file has no parent directory"))?,
    )?;
    Ok(real)
}

fn check(invocation: &Invocation<'_>) -> Result<()> {
    invocation
        .context
        .check(invocation.clock)
        .map_or(Ok(()), |end| Err(greennode_sdk::Error::from(end).into()))
}

async fn execute(
    invocation: &mut Invocation<'_>,
    profile: &str,
    cwd: Option<&Path>,
    flag: bool,
    (key, credentials, secret): (&str, bool, bool),
    argument: &OsStr,
) -> Result<()> {
    check(invocation)?;
    let snapshot = Snapshot::read(invocation.source, cwd, profile, flag)?;
    let mut buffer = Zeroizing::new([0; STDIN_LIMIT + 1]);
    let value = if argument == "-" {
        if invocation.stdin_terminal {
            return Err(Error::Usage(format!(
                "configure set {key} - refuses a terminal on stdin; use configure instead"
            )));
        }
        let value = read_stdin(invocation.stdin, &mut buffer);
        check(invocation)?;
        value?
    } else {
        if secret {
            return Err(Error::Usage(format!(
                "{key} must be set from stdin: configure set {key} -"
            )));
        }
        argument
            .to_str()
            .ok_or_else(|| Error::Usage("value is not valid UTF-8".to_owned()))?
    };
    validate(key, value)?;

    tokio::task::yield_now().await;
    check(invocation)?;
    snapshot.legacy.recheck()?;
    let [config, credential_path] = &snapshot.paths;
    let [config_slot, credential_slot] = snapshot.slots;
    let config = files::Target::open(config, config_slot, &snapshot.legacy)?;
    let credentials = credentials
        .then(|| files::Target::open(credential_path, credential_slot, &snapshot.legacy))
        .transpose()?;
    let mut lock_targets: Vec<_> = std::iter::once(&config)
        .chain(credentials.as_ref())
        .collect();
    lock_targets.sort_unstable_by(|left, right| left.lock_path.cmp(&right.lock_path));
    if lock_targets
        .iter()
        .any(|target| snapshot.paths.contains(&target.lock_path))
    {
        return Err(Error::Config(
            "configuration file conflicts with a configure lock",
        ));
    }
    let mut locks: Vec<files::Lock<'_>> = Vec::new();
    for target in lock_targets {
        let lock = target.lock(&snapshot.legacy)?;
        if locks
            .iter()
            .all(|previous| previous.identity != lock.identity)
        {
            loop {
                check(invocation)?;
                if lock.try_acquire()? {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        locks.push(lock);
    }
    let guard = || -> Result<Snapshot> {
        let current = snapshot.recheck(invocation, cwd, profile, flag)?;
        for lock in &locks {
            lock.verify(&snapshot.legacy)?;
        }
        Ok(current)
    };
    let current = guard()?;
    let target = credentials.as_ref().unwrap_or(&config);
    let section = if credentials.is_none() && profile != "default" {
        format!("profile {profile}")
    } else {
        profile.to_owned()
    };
    let present = if target.slot.slot == FileSlot::Config {
        current.present[0]
    } else {
        current.present[1]
    };
    let contents = target.read(present)?;
    let edit = ini::Edit::new(&contents.bytes, target.slot, &section, key, value)?;
    let mut pending = target.prepare(&snapshot.legacy, |file| edit.write(file))?;
    tokio::task::yield_now().await;
    pending
        .publish(|| {
            guard()?;
            target.verify_original(&contents)
        })
        .await
}

fn read_stdin<'a>(reader: &mut dyn Read, buffer: &'a mut [u8; STDIN_LIMIT + 1]) -> Result<&'a str> {
    let mut used = 0;
    while let Some(rest) = buffer.get_mut(used..).filter(|rest| !rest.is_empty()) {
        match reader.read(rest) {
            Ok(0) => break,
            Ok(n) => used += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Usage("reading stdin failed".to_owned())),
        }
    }
    let bytes = buffer
        .get(..used)
        .filter(|bytes| bytes.len() <= STDIN_LIMIT)
        .ok_or_else(|| Error::Usage("stdin value is larger than 4096 bytes".to_owned()))?;
    let bytes = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes);
    if bytes.is_empty() {
        return Err(Error::Usage("stdin had no value".to_owned()));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| Error::Usage("stdin value is not valid UTF-8".to_owned()))
}

fn validate(key: &str, value: &str) -> Result<()> {
    if value.contains(['\r', '\n']) {
        return Err(Error::Usage(
            "configure value contains a newline".to_owned(),
        ));
    }
    let failure = match key {
        "region" if !value.is_empty() => (!valid_region(value))
            .then_some("region must be a lowercase DNS label of at most 63 bytes"),
        "read_only" if latch(value).is_none() => {
            Some("read_only must be 1, true, 0, false, or empty")
        }
        "output" if !matches!(value, "" | "json" | "table" | "text") => {
            Some("output must be json, table, or text")
        }
        "vcdn_api_key" if value.chars().any(|c| c.is_whitespace() || c.is_control()) => {
            Some("vcdn_api_key must not hold whitespace or a control character")
        }
        _ => None,
    };
    failure.map_or(Ok(()), |message| Err(Error::Usage(message.to_owned())))
}

fn valid_region(region: &str) -> bool {
    region.len() <= 63
        && !region.starts_with('-')
        && !region.ends_with('-')
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
