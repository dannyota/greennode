use std::path::PathBuf;
use std::process::{Command, ExitCode, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Home(PathBuf);

impl greennode_sdk::ConfigSource for Home {
    fn var_os(&self, _: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
        None
    }

    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
}

impl Home {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "greennode-configure-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_greennode"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    #[cfg(not(windows))]
    fn with_stdin(&self, args: &[&str], value: &[u8]) -> Output {
        let input = self.0.join("stdin");
        std::fs::write(&input, value).unwrap();
        let file = std::fs::File::open(&input).unwrap();
        let result = self.command(args).stdin(file).output().unwrap();
        std::fs::remove_file(input).unwrap();
        result
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn interactive_configure_requires_terminal_stdin() {
    let home = Home::new();
    let output = home.run(&["configure"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("InvalidUsage"));
    assert!(error.contains("configure set <key> -"));
    assert!(!home.0.join(".greennode").exists());
}

#[cfg(not(windows))]
#[test]
fn legacy_write_refuses_without_creating_files() {
    let home = Home::new();
    let legacy = home.0.join(".vngcloud");
    std::fs::create_dir(&legacy).unwrap();
    let config = legacy.join("config");
    std::fs::write(&config, "[default]\nregion = han-1\n").unwrap();
    let output = home.run(&["configure", "set", "region", "hcm-3"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains(".vngcloud"));
    assert!(error.contains(".greennode"));
    assert!(error.contains("no files changed"));
    assert!(!home.0.join(".greennode").exists());
    assert_eq!(
        std::fs::read(config).unwrap(),
        b"[default]\nregion = han-1\n"
    );
    assert_eq!(std::fs::read_dir(legacy).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn fresh_set_creates_private_config_and_get_returns_value() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let output = home.run(&["configure", "set", "region", "han-1"]);
    assert!(output.status.success());
    let directory = home.0.join(".greennode");
    let config = directory.join("config");
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let output = home.run(&["configure", "get", "region"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"han-1\n");
}

#[cfg(unix)]
#[test]
fn get_refuses_group_readable_credentials_without_disclosing_value() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let credentials = directory.join("credentials");
    std::fs::write(&credentials, "[default]\npassword = toy-canary\n").unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o640)).unwrap();
    let output = home.run(&["configure", "get", "password"]);
    assert_eq!(output.status.code(), Some(3));
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("credentials"));
    assert!(diagnostic.contains("~/.greennode"));
    assert!(diagnostic.contains("chmod 600"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("toy-canary"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("toy-canary"));
}

#[cfg(unix)]
#[test]
fn get_masks_stored_secret() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let credentials = directory.join("credentials");
    std::fs::write(&credentials, "[default]\npassword = toy-canary\n").unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = home.run(&["configure", "get", "password"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"****\n");
    assert!(output.stderr.is_empty());
}

#[cfg(unix)]
#[test]
fn list_reads_legacy_profiles_in_order_and_masks_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let directory = home.0.join(".vngcloud");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("config"),
        "[profile dev]\nregion = han-1\nproject_id = a\x1bb\nread_only = true\n",
    )
    .unwrap();
    let credentials = directory.join("credentials");
    std::fs::write(
        &credentials,
        "[dev]\nusername = toy-user\npassword = toy-canary\nclient_secret = toy-secret\n",
    )
    .unwrap();
    std::fs::set_permissions(credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = home.run(&[
        "--profile",
        "dev",
        "--read-only",
        "--output",
        "json",
        "--query",
        "bad[",
        "configure",
        "list",
    ]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "region = han-1\nproject_id = a\\x1bb\noutput = \nread_only = true\nroot_email = \nusername = toy-user\npassword = ****\ntotp_secret = \nvcdn_api_key = \nclient_id = \nclient_secret = ****\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_values_and_region_hint() {
    let home = Home::new();
    let get = home.run(&["configure", "get", "region"]);
    assert!(get.status.success());
    assert_eq!(get.stdout, b"\n");
    let list = home.run(&["configure", "list"]);
    assert!(list.status.success());
    let error = String::from_utf8(list.stderr).unwrap();
    assert!(error.contains(".greennode/config"));
    assert!(error.contains("GREENNODE_REGION"));
}

#[test]
fn missing_home_requires_override_for_requested_slot() {
    for args in [
        &["configure", "get", "region"][..],
        &["configure", "get", "password"][..],
        &["configure", "list"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_greennode"))
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn config_get_does_not_open_insecure_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("config"), "[default]\nregion = han-1\n").unwrap();
    let credentials = directory.join("credentials");
    std::fs::write(&credentials, "[default]\npassword = toy-canary\n").unwrap();
    std::fs::set_permissions(credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = home.run(&["configure", "get", "region"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"han-1\n");
    let output = home.run(&["configure", "list"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
}

#[test]
fn get_help_does_not_require_key() {
    let home = Home::new();
    let output = home.run(&["configure", "get", "--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    assert!(output.stderr.is_empty());
    assert_eq!(home.run(&["configure", "get"]).status.code(), Some(2));
}

#[test]
fn set_help_does_not_require_key_or_value() {
    let home = Home::new();
    for flag in ["--help", "-h"] {
        let output = home.run(&["configure", "set", flag]);
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(home.run(&["configure", "set"]).status.code(), Some(2));
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[test]
fn set_rejects_unknown_flag_in_value_position() {
    let home = Home::new();
    let output = home.run(&["configure", "set", "project_id", "--typo"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("InvalidUsage") && error.contains("unknown argument or flag"));
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[cfg(not(windows))]
#[test]
fn set_accepts_flag_like_value_after_separator() {
    let home = Home::new();
    let output = home.run(&["configure", "set", "project_id", "--", "--typo"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let output = home.run(&["configure", "get", "project_id"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"--typo\n");
}

#[cfg(windows)]
#[test]
fn windows_configure_writes_refuse_before_reading_or_creating_files() {
    let home = Home::new();
    for args in [
        &["configure"][..],
        &["configure", "set", "region", "han-1"],
        &["configure", "set", "password", "-"],
        &["configure", "set", "project_id", "--", "--typo"],
    ] {
        let (exit, stdout, stderr) = invoke_configure(&home, args, &mut Unreadable, false);
        assert_eq!(exit, ExitCode::from(2));
        assert!(stdout.is_empty());
        let error = String::from_utf8_lossy(&stderr);
        assert!(error.contains("InvalidUsage"));
        assert!(error.contains("configure writes are not supported on Windows yet"));
        assert!(error.contains("Edit the files by hand"));
        assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
    }
}

#[test]
fn invalid_profile_reports_usage_and_syntax() {
    let output = Home::new().run(&["--profile", "bad[name", "configure", "get", "region"]);
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("InvalidUsage"));
    assert!(error.contains("leading or trailing whitespace"));
}

#[test]
fn malformed_config_diagnostic_includes_line() {
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("config"), "[default]\ninvalid\n").unwrap();
    let output = home.run(&["configure", "get", "region"]);
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("config (~/.greennode):2"));
    assert!(error.contains("line has no '='"));
}

#[cfg(unix)]
#[test]
fn legacy_and_read_only_refuse_before_consuming_stdin() {
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    for (directory, message) in [
        (".vngcloud", "no files changed"),
        (".greennode", "ReadOnly"),
    ] {
        let home = Home::new();
        let selected = home.0.join(directory);
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("config"), "[default]\nread_only = true\n").unwrap();
        let (mut input, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"toy-canary\n").unwrap();
        drop(writer);
        let child_input = OwnedFd::from(input.try_clone().unwrap());
        let output = home
            .command(&["configure", "set", "password", "-"])
            .stdin(Stdio::from(child_input))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        assert!(output.stdout.is_empty());
        let mut remaining = Vec::new();
        input.read_to_end(&mut remaining).unwrap();
        assert!(remaining == b"toy-canary\n");
        assert_eq!(std::fs::read_dir(&selected).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1);
    }
}

#[cfg(not(windows))]
#[test]
fn latch_sources_are_validated_and_cannot_be_cleared() {
    let home = Home::new();
    let args = [
        "--read-only=false",
        "configure",
        "set",
        "read_only",
        "false",
    ];
    let output = home
        .command(&args)
        .env("VNGCLOUD_READ_ONLY", " TrUe ")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("VNGCLOUD_READ_ONLY"));
    let output = home
        .command(&args)
        .env("GREENNODE_READ_ONLY", "true")
        .env("VNGCLOUD_READ_ONLY", "invalid")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("InvalidConfig"));
    assert!(!home.0.join(".greennode").exists());
}

#[cfg(not(windows))]
#[test]
fn literal_secrets_are_refused_without_disclosure() {
    let home = Home::new();
    for key in ["password", "totp_secret", "vcdn_api_key", "client_secret"] {
        let output = home.run(&["configure", "set", key, "toy-canary"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("must be set from stdin"));
        assert!(!error.contains("toy-canary"));
    }
    assert!(!home.0.join(".greennode").exists());
}

#[cfg(not(windows))]
#[test]
fn stdin_strips_only_one_line_ending_and_enforces_the_byte_limit() {
    let home = Home::new();
    let args = ["configure", "set", "password", "-"];
    let mut at_limit = vec![b'x'; 4094];
    at_limit.extend_from_slice(b"\r\n");
    let over_limit = vec![b'x'; 4097];
    for (value, message) in [
        (&b"\n"[..], "stdin had no value"),
        (&b"toy-canary\n\n"[..], "newline"),
        (&b"toy-canary\rtext"[..], "newline"),
        (&over_limit[..], "larger than 4096 bytes"),
    ] {
        let output = home.with_stdin(&args, value);
        assert_eq!(output.status.code(), Some(2));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(message));
        assert!(!error.contains("toy-canary"));
        assert!(output.stdout.is_empty());
        assert!(!home.0.join(".greennode").exists());
    }
    let output = home.with_stdin(&args, &at_limit);
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let contents = std::fs::read(home.0.join(".greennode/credentials")).unwrap();
    let expected = [b"[default]\npassword = ", &at_limit[..4094], b"\n"].concat();
    assert!(contents == expected);
    let output = home.run(&["configure", "get", "password"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"****\n");
}

#[cfg(unix)]
#[test]
fn credentials_and_created_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let home = Home::new();
    let output = home.with_stdin(&["configure", "set", "password", "-"], b"toy-canary\n");
    assert!(output.status.success());
    let directory = home.0.join(".greennode");
    for (path, mode) in [
        (directory.clone(), 0o700),
        (directory.join("credentials"), 0o600),
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    assert!(directory.join("config.lock").is_file());
    assert!(directory.join("credentials.lock").is_file());
    assert!(!directory.join("config").exists());
}

#[cfg(not(windows))]
#[test]
fn editing_last_duplicate_preserves_bom_comments_and_other_sections() {
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("config");
    let before = "\u{feff}; top\r\n[profile team]\r\nregion = han-2\r\n# keep\r\n[default]\r\nregion = han-1\r\n[profile team]\r\n  ReGiOn = old\r\n; tail\r\n";
    std::fs::write(&config, before).unwrap();
    let output = home.run(&["--profile", "team", "configure", "set", "region", "hcm-3"]);
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(config).unwrap(),
        before.replace("= old", "= hcm-3")
    );
    let output = home.run(&["--profile", "team", "configure", "get", "region"]);
    assert_eq!(output.stdout, b"hcm-3\n");
}

#[cfg(unix)]
#[test]
fn symlinked_write_target_into_real_legacy_tree_is_refused() {
    use std::os::unix::fs::symlink;

    let home = Home::new();
    let legacy = home.0.join("renamed-legacy");
    std::fs::create_dir(&legacy).unwrap();
    symlink(&legacy, home.0.join(".vngcloud")).unwrap();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(legacy.join("config"), "[default]\nregion = han-1\n").unwrap();
    symlink(legacy.join("config"), directory.join("config")).unwrap();
    std::fs::write(directory.join("credentials"), "").unwrap();
    let output = home.run(&["configure", "set", "region", "hcm-3"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("legacy"));
    assert_eq!(
        std::fs::read(legacy.join("config")).unwrap(),
        b"[default]\nregion = han-1\n"
    );
    assert_eq!(std::fs::read_dir(&legacy).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
}

#[cfg(not(windows))]
#[test]
fn latch_change_after_snapshot_refuses_publication() {
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("config");
    std::fs::write(&config, "[default]\nread_only = false\n").unwrap();
    let mut input = ChangeBeforeInput(Some(|| {
        std::fs::write(&config, "[default]\nread_only = true\n")
    }));
    let (exit, stdout, stderr) = invoke_set(&home, &mut input, false);
    assert_eq!(exit, ExitCode::from(2));
    assert!(input.0.is_none());
    assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
    assert!(stdout.is_empty());
    assert!(!directory.join("credentials").exists());
    assert!(directory.join("config.lock").is_file());
    assert!(directory.join("credentials.lock").is_file());
    assert_eq!(
        std::fs::read(config).unwrap(),
        b"[default]\nread_only = true\n"
    );
}

#[cfg(not(windows))]
struct ChangeBeforeInput<F>(Option<F>);

#[cfg(not(windows))]
impl<F: FnOnce() -> std::io::Result<()>> std::io::Read for ChangeBeforeInput<F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(change) = self.0.take() else {
            return Ok(0);
        };
        change()?;
        (&b"toy-canary\n"[..]).read(buffer)
    }
}

#[cfg(unix)]
#[test]
fn config_alias_changed_after_snapshot_refuses_a_credentials_edit() {
    use std::os::unix::fs::symlink;
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let first = home.0.join("first");
    let second = home.0.join("second");
    for path in [&first, &second] {
        std::fs::write(path, "[default]\n").unwrap();
    }
    let config = directory.join("config");
    symlink(&first, &config).unwrap();
    let mut input = ChangeBeforeInput(Some(|| {
        std::fs::remove_file(&config)?;
        symlink(&second, &config)
    }));

    let (exit, stdout, stderr) = invoke_set(&home, &mut input, false);
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("configuration paths changed"));
    assert!(home.0.join("first.lock").is_file());
    assert!(!home.0.join("second.lock").exists());
    assert!(!directory.join("credentials").exists());
}

#[cfg(unix)]
#[test]
fn in_place_change_after_read_refuses_publication() {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::AtomicBool;

    struct EditBeforePublish<'a> {
        home: &'a Home,
        edited: AtomicBool,
    }

    impl greennode_sdk::ConfigSource for EditBeforePublish<'_> {
        fn var_os(&self, name: &OsStr) -> Option<OsString> {
            let directory = self.home.0.join(".greennode");
            if name == "GREENNODE_READ_ONLY"
                && !self.edited.load(Ordering::Relaxed)
                && std::fs::read_dir(&directory)
                    .unwrap()
                    .any(|entry| entry.unwrap().path().extension() == Some(OsStr::new("tmp")))
            {
                std::fs::write(directory.join("config"), "[default]\nregion = han-2\n").unwrap();
                self.edited.store(true, Ordering::Relaxed);
            }
            None
        }

        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.home.0.clone())
        }
    }

    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("config");
    std::fs::write(&config, "[default]\nregion = han-1\n").unwrap();
    let inode = std::fs::metadata(&config).unwrap().ino();
    let source = EditBeforePublish {
        home: &home,
        edited: AtomicBool::new(false),
    };
    let (exit, stdout, stderr) = invoke_configure(
        &source,
        &["configure", "set", "region", "hcm-3"],
        &mut Unreadable,
        false,
    );

    assert!(source.edited.load(Ordering::Relaxed));
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("file changed during the edit"));
    assert_eq!(
        std::fs::read(&config).unwrap(),
        b"[default]\nregion = han-2\n"
    );
    assert_eq!(std::fs::metadata(config).unwrap().ino(), inode);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 2);
}

#[cfg(not(windows))]
fn invoke_set(
    home: &Home,
    input: &mut dyn std::io::Read,
    terminal: bool,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    invoke_configure(
        home,
        &["configure", "set", "password", "-"],
        input,
        terminal,
    )
}

fn invoke_configure(
    source: &dyn greennode_sdk::ConfigSource,
    args: &[&str],
    input: &mut dyn std::io::Read,
    terminal: bool,
) -> (ExitCode, Vec<u8>, Vec<u8>) {
    use greennode::cli::{Invocation, run_invocation};
    use greennode_sdk::{Clock, RequestContext, SystemClock};
    use std::ffi::OsString;
    use std::time::Duration;
    let (context, _) = RequestContext::new();
    let context = context.child_with_deadline(SystemClock.monotonic_now() + Duration::from_secs(5));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let exit = runtime.block_on(run_invocation(Invocation {
        args: std::iter::once("greennode")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect(),
        source,
        config_options: greennode_sdk::ConfigOptions::default(),
        stdin: &mut { input },
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_terminal: terminal,
        stdout_terminal: false,
        stderr_terminal: false,
        clock: &SystemClock,
        context: &context,
    }));
    (exit, stdout, stderr)
}

struct Unreadable;

impl std::io::Read for Unreadable {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("stdin was read");
    }
}

#[cfg(not(windows))]
#[test]
fn terminal_stdin_is_refused_before_reading() {
    let home = Home::new();
    let (exit, stdout, stderr) = invoke_set(&home, &mut Unreadable, true);
    assert_eq!(exit, ExitCode::from(2));
    assert!(stdout.is_empty());
    assert!(String::from_utf8_lossy(&stderr).contains("refuses a terminal on stdin"));
    assert!(!home.0.join(".greennode").exists());
}

#[cfg(unix)]
#[test]
fn hard_linked_lock_files_are_acquired_once() {
    let home = Home::new();
    let directory = home.0.join(".greennode");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("config.lock"), []).unwrap();
    std::fs::hard_link(
        directory.join("config.lock"),
        directory.join("credentials.lock"),
    )
    .unwrap();

    let (exit, stdout, stderr) = invoke_set(&home, &mut &b"toy-canary\n"[..], false);
    assert_eq!(exit, ExitCode::SUCCESS);
    assert!(stdout.is_empty() && stderr.is_empty());
    assert!(directory.join("credentials").is_file());
}

#[cfg(unix)]
#[test]
fn sigterm_cancels_set_while_stdin_is_waiting_for_eof() {
    use rustix::process::{Pid, Signal, kill_process};
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let home = Home::new();
    let (input, mut writer) = UnixStream::pair().unwrap();
    writer.write_all(b"x").unwrap();
    let child_input = OwnedFd::from(input.try_clone().unwrap());
    let mut child = home
        .command(&["configure", "set", "password", "-"])
        .stdin(Stdio::from(child_input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while rustix::io::ioctl_fionread(&input).unwrap() != 0 {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not begin reading stdin");
        }
        std::thread::yield_now();
    }
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    kill_process(pid, Signal::TERM).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not cancel its stdin read");
        }
        std::thread::yield_now();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!home.0.join(".greennode").exists());
    drop(writer);
}

#[cfg(unix)]
mod interactive {
    use super::*;
    use greennode::cli::{Invocation, Terminal, run_invocation_with_terminal};
    use greennode_sdk::{RequestContext, SystemClock};
    use std::cell::Cell;
    use std::io::{self, Read};
    use std::os::unix::fs::PermissionsExt;

    struct Echo<'a> {
        enabled: &'a Cell<bool>,
        changes: Vec<bool>,
    }

    impl Terminal for Echo<'_> {
        fn disable_echo(&mut self) -> io::Result<()> {
            self.enabled.set(false);
            self.changes.push(false);
            Ok(())
        }

        fn restore_echo(&mut self, _: bool) -> io::Result<()> {
            self.enabled.set(true);
            self.changes.push(true);
            Ok(())
        }
    }

    struct Script<'a, F> {
        echo: &'a Cell<bool>,
        answers: &'a [&'a [u8]],
        on_read: F,
        step: usize,
        line: &'a [u8],
    }

    impl<F: FnMut(usize)> Read for Script<'_, F> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.line.is_empty() {
                assert_eq!(self.echo.get(), self.step < 6);
                (self.on_read)(self.step);
                self.line = self.answers[self.step];
                self.step += 1;
            }
            self.line.read(output)
        }
    }

    fn run(
        source: &dyn greennode_sdk::ConfigSource,
        context: &RequestContext,
        answers: &[&[u8]],
        on_read: impl FnMut(usize),
    ) -> (ExitCode, Vec<u8>, Vec<u8>, Vec<bool>) {
        let echo = Cell::new(true);
        let mut terminal = Echo {
            enabled: &echo,
            changes: Vec::new(),
        };
        let mut input = Script {
            echo: &echo,
            answers,
            on_read,
            step: 0,
            line: &[],
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let invocation = Invocation {
            args: ["greennode", "configure"].map(Into::into).to_vec(),
            source,
            config_options: greennode_sdk::ConfigOptions::default(),
            stdin: &mut (&mut input as &mut dyn std::io::Read),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_terminal: true,
            stdout_terminal: false,
            stderr_terminal: false,
            clock: &SystemClock,
            context,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let exit = runtime.block_on(run_invocation_with_terminal(invocation, &mut terminal));
        (exit, stdout, stderr, terminal.changes)
    }

    #[test]
    fn prompts_in_baseline_order_and_publishes_only_nonblank_values() {
        let home = Home::new();
        let directory = home.0.join(".greennode");
        std::fs::create_dir(&directory).unwrap();
        let config = directory.join("config");
        let credentials = directory.join("credentials");
        std::fs::write(
            &config,
            b"[default]\nregion = han-1\nproject_id = toy-project\noutput = text\n",
        )
        .unwrap();
        std::fs::write(
            &credentials,
            b"[default]\nusername = toy-user\npassword = toy-old-password\ntotp_secret = toy-old-totp\n",
        )
        .unwrap();
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();

        let (context, _) = RequestContext::new();
        let (exit, stdout, stderr, changes) = run(
            &home,
            &context,
            &[
                b"han-2\r\n",
                b"\n",
                b"json\n",
                b"true\n",
                b"toy@example.invalid\n",
                b"\n",
                b"\n",
                b"toy-new-totp\n",
            ],
            |_| {},
        );

        assert_eq!(exit, ExitCode::SUCCESS);
        for secret in ["toy-old-password", "toy-old-totp", "toy-new-totp"] {
            assert!(!String::from_utf8_lossy(&stdout).contains(secret));
            assert!(!String::from_utf8_lossy(&stderr).contains(secret));
        }
        assert_eq!(
            stdout,
            b"Region [han-1]: Project ID [toy-project]: Output format [text]: Read only []: Root email []: Username [toy-user]: Password [****]: \nTOTP secret [****]: \n"
        );
        assert!(stderr.is_empty());
        assert_eq!(changes, [false, true, false, true]);
        assert_eq!(
            std::fs::read(config).unwrap(),
            b"[default]\nregion = han-2\nproject_id = toy-project\noutput = json\nread_only = true\n"
        );
        assert!(
            std::fs::read(credentials).unwrap()
                == b"[default]\nusername = toy-user\npassword = toy-old-password\ntotp_secret = toy-new-totp\nroot_email = toy@example.invalid\n"
        );
    }

    #[test]
    fn legacy_mode_refuses_before_the_first_prompt() {
        let home = Home::new();
        let legacy = home.0.join(".vngcloud");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("config"), b"[default]\nregion = han-1\n").unwrap();
        let (context, _) = RequestContext::new();
        let (exit, stdout, stderr, changes) = run(&home, &context, &[], |_| {});
        assert_eq!(exit, ExitCode::from(2));
        assert!(String::from_utf8_lossy(&stderr).contains("legacy configuration is read-only"));
        assert!(stdout.is_empty() && changes.is_empty());
        assert_eq!(std::fs::read_dir(legacy).unwrap().count(), 1);
        assert!(!home.0.join(".greennode").exists());
    }

    #[test]
    fn profile_latch_refuses_before_the_first_prompt() {
        let home = Home::new();
        let directory = home.0.join(".greennode");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("config"), b"[default]\nread_only = true\n").unwrap();
        let (context, _) = RequestContext::new();
        let (exit, stdout, stderr, changes) = run(&home, &context, &[], |_| {});
        assert_eq!(exit, ExitCode::from(2));
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
        assert!(stdout.is_empty() && changes.is_empty());
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn latch_change_during_a_prompt_refuses_both_edits_under_lock() {
        use std::fs::{File, TryLockError};
        use std::sync::atomic::AtomicBool;

        struct Source<'a> {
            home: &'a Home,
            changed: AtomicBool,
        }

        impl greennode_sdk::ConfigSource for Source<'_> {
            fn var_os(&self, _: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
                None
            }

            fn home_dir(&self) -> Option<PathBuf> {
                if self.changed.load(Ordering::Relaxed) {
                    for name in ["config.lock", "credentials.lock"] {
                        let file = File::open(self.home.0.join(".greennode").join(name)).unwrap();
                        assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
                    }
                }
                Some(self.home.0.clone())
            }
        }

        let home = Home::new();
        let source = Source {
            home: &home,
            changed: AtomicBool::new(false),
        };
        let directory = home.0.join(".greennode");
        std::fs::create_dir(&directory).unwrap();
        let config = directory.join("config");
        std::fs::write(&config, b"[default]\nread_only = false\n").unwrap();
        let (context, _) = RequestContext::new();
        let (exit, _, stderr, _) = run(
            &source,
            &context,
            &[
                b"han-2\n",
                b"\n",
                b"\n",
                b"\n",
                b"\n",
                b"\n",
                b"toy-secret\n",
                b"\n",
            ],
            |step| {
                if step == 6 {
                    assert!(!directory.join("config.lock").exists());
                    std::fs::write(&config, b"[default]\nread_only = true\n").unwrap();
                    source.changed.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(exit, ExitCode::from(2));
        assert!(String::from_utf8_lossy(&stderr).contains("ReadOnly"));
        assert!(!String::from_utf8_lossy(&stderr).contains("toy-secret"));
        assert!(directory.join("config.lock").is_file());
        assert!(directory.join("credentials.lock").is_file());
        assert!(!directory.join("credentials").exists());
        assert_eq!(
            std::fs::read(config).unwrap(),
            b"[default]\nread_only = true\n"
        );
    }

    #[test]
    fn cancellation_during_a_secret_prompt_restores_echo() {
        let home = Home::new();
        let (context, cancel) = RequestContext::new();
        let (exit, stdout, stderr, changes) = run(
            &home,
            &context,
            &[
                b"han-2\n",
                b"\n",
                b"\n",
                b"\n",
                b"\n",
                b"\n",
                b"toy-cancelled\n",
            ],
            |step| {
                if step == 6 {
                    cancel.cancel();
                }
            },
        );
        assert_eq!(exit, ExitCode::from(1));
        assert_eq!(changes, [false, true]);
        assert!(!String::from_utf8_lossy(&stdout).contains("toy-cancelled"));
        assert!(!String::from_utf8_lossy(&stderr).contains("toy-cancelled"));
        assert!(!home.0.join(".greennode").exists());
    }
}
