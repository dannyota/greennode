use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Home(PathBuf);

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

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_greennode"))
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .output()
            .unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "configure set lands in the next step"]
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
#[ignore = "configure set lands in the next step"]
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
