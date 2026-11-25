use super::*;
use std::env;

struct Dir(std::path::PathBuf);

impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-config-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn windows_open_uses_identification_security_qos() {
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    let dir = Dir::new();
    let path = dir.0.join("regular");
    std::fs::write(&path, b"ok").unwrap();
    let file = open_windows(&path, |options, path| {
        let expected = SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT;
        assert!(format!("{options:?}").contains(&format!("security_qos_flags: {expected}")));
        options.open(path)
    });
    assert!(file.is_ok());
}

#[cfg(windows)]
#[test]
fn windows_refuses_non_disk_handles_before_metadata() {
    let dir = Dir::new();
    let path = dir.0.join("regular");
    std::fs::write(&path, b"ok").unwrap();
    assert!(open(&path, Slot::Config).is_ok());
    let error = open(Path::new("NUL"), Slot::Config).err().unwrap();
    assert_eq!(error, "config: not a regular file");
}

#[test]
fn rejects_non_regular_files() {
    let dir = Dir::new();
    let regular = dir.0.join("regular");
    std::fs::write(&regular, b"ok").unwrap();
    assert!(open(&regular, Slot::Config).is_ok());
    assert!(open(&dir.0, Slot::Config).is_err());
}

#[test]
fn rejects_oversized_opened_file() {
    let dir = Dir::new();
    let path = dir.0.join("file");
    let file = File::create(&path).unwrap();
    file.set_len(1_048_576).unwrap();
    assert!(open(&path, Slot::Config).is_ok());
    file.set_len(1_048_577).unwrap();
    assert!(open(&path, Slot::Config).is_err());
}

#[test]
fn detects_file_identity_from_open_handles() {
    let dir = Dir::new();
    let first = dir.0.join("first");
    let alias = dir.0.join("alias");
    let other = dir.0.join("other");
    std::fs::write(&first, b"one").unwrap();
    std::fs::hard_link(&first, &alias).unwrap();
    std::fs::write(&other, b"two").unwrap();
    let first = open(&first, Slot::Config).unwrap();
    let alias = open(&alias, Slot::Config).unwrap();
    let other = open(&other, Slot::Config).unwrap();
    assert!(same(&first, &alias).unwrap());
    assert!(!same(&first, &other).unwrap());
}

#[cfg(all(
    unix,
    not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "l4re",
        target_os = "redox",
        target_os = "vita"
    ))
))]
#[test]
fn unix_open_flags_prevent_controlling_terminal_acquisition() {
    assert!(open_flags().contains(rustix::fs::OFlags::NOCTTY));
    assert!(open_flags().contains(rustix::fs::OFlags::NONBLOCK));
}

#[cfg(unix)]
#[test]
fn checks_credentials_permissions_on_target() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let path = dir.0.join("file");
    std::fs::write(&path, b"ok").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(open(&path, Slot::Credentials).is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(open(&path, Slot::Credentials).is_ok());
    for mode in [0o620, 0o601] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(open(&path, Slot::Config).is_ok());
        assert!(open(&path, Slot::Credentials).is_err());
    }
}

#[cfg(unix)]
#[test]
fn follows_symlink_and_refuses_fifo() {
    use std::os::unix::fs::symlink;
    let dir = Dir::new();
    let regular = dir.0.join("regular");
    let link = dir.0.join("link");
    let fifo = dir.0.join("fifo");
    std::fs::write(&regular, b"ok").unwrap();
    symlink(&regular, &link).unwrap();
    assert!(open(&link, Slot::Config).is_ok());
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(open(&fifo, Slot::Config).is_err());
}
