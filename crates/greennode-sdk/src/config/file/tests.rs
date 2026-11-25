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
fn config_open_error_keeps_safe_source_and_reason() {
    use crate::config::credentials::FileDiagnostic;
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config-canary");
    File::create(&config).unwrap().set_len(1_048_577).unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config.clone()),
            credentials: None,
            config_named: true,
            credentials_named: false,
        },
        config_present: true,
        credentials_present: false,
        config_source: Some("GREENNODE_CONFIG_FILE"),
        credentials_source: None,
    };
    let error = open_pair(&selection).err().unwrap();
    assert_eq!(error.class(), crate::ErrorClass::InvalidConfig);
    let detail = error.detail::<FileDiagnostic>();
    assert!(detail.is_some());
    let detail = detail.unwrap();
    assert_eq!(detail.source(), "GREENNODE_CONFIG_FILE");
    assert_eq!(detail.reason(), "file too large");
    assert!(!format!("{detail:?}").contains("config-canary"));
    std::fs::remove_file(&config).unwrap();
    let error = open_pair(&selection).err().unwrap();
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.source(), "GREENNODE_CONFIG_FILE");
    assert_eq!(detail.reason(), "open failed");
    assert!(!format!("{detail:?}").contains("config-canary"));
    std::fs::write(&config, "[default]\n").unwrap();
    assert!(open_pair(&selection).is_ok());
    std::fs::remove_file(&config).unwrap();
    let mut selection = selection;
    selection.config_present = false;
    let error = open_pair(&selection).err().unwrap();
    assert_eq!(error.class(), crate::ErrorClass::InvalidConfig);
    let detail = error.detail::<FileDiagnostic>();
    assert!(detail.is_some());
    let detail = detail.unwrap();
    assert_eq!(detail.source(), "GREENNODE_CONFIG_FILE");
    assert_eq!(detail.reason(), "not found");
    assert!(!format!("{detail:?}").contains("config-canary"));
}

#[test]
fn ended_workers_stop_before_credentials_open() {
    use crate::config::mode::{Mode, Paths, Selection};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\n").unwrap();
    std::fs::write(&credentials, "[default]\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config),
            credentials: Some(credentials),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    for state in [2, 0, 1, 3] {
        let (ctx, cancel) = crate::RequestContext::new();
        let expired = ctx.child_with_deadline(tokio::time::Instant::now());
        let dropped = AtomicBool::new(false);
        let started = Cell::new(false);
        let opens = Cell::new(0);
        let result = open_pair_guarded(
            &selection,
            || {
                if state == 1 && started.get() {
                    expired.check(&crate::SystemClock)
                } else {
                    ctx.check(&crate::SystemClock)
                }
                .or_else(|| {
                    dropped
                        .load(Ordering::Acquire)
                        .then_some(crate::ContextEnd::Cancelled)
                })
            },
            || {
                started.set(true);
                if state == 0 {
                    cancel.cancel();
                }
                if state == 2 {
                    dropped.store(true, Ordering::Release);
                }
            },
            || opens.set(opens.get() + 1),
        );
        if state == 3 {
            assert!(result.is_ok());
            assert_eq!(opens.get(), 1);
        } else {
            let expected = if state == 1 {
                crate::ErrorClass::DeadlineExceeded
            } else {
                crate::ErrorClass::Cancelled
            };
            assert!(matches!(result, Err(error) if error.class() == expected));
            assert_eq!(opens.get(), 0);
        }
    }
}

#[test]
fn opened_pair_rejects_hard_link_alias() {
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, b"[unfinished").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::hard_link(&config, &credentials).unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config),
            credentials: Some(credentials.clone()),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    let alias_error = open_pair(&selection).err().unwrap();
    assert_eq!(alias_error.class(), crate::ErrorClass::InvalidConfig);
    std::fs::remove_file(&credentials).unwrap();
    std::fs::write(&credentials, b"[default]\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let result = open_pair(&selection);
    assert!(result.is_ok());
    let pair = result.ok().unwrap();
    assert!(pair.config.is_some() && pair.credentials.is_some());
}

#[cfg(unix)]
#[test]
fn unsafe_credentials_report_source_and_permission_guidance() {
    use crate::config::credentials::FileDiagnostic;
    use crate::config::mode::{Mode, Paths, Selection};
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let credentials = dir.0.join("credentials");
    std::fs::write(&credentials, b"[default]\n").unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Legacy,
            config: None,
            credentials: Some(credentials.clone()),
            config_named: false,
            credentials_named: true,
        },
        config_present: false,
        credentials_present: true,
        config_source: None,
        credentials_source: Some("VNGCLOUD_SHARED_CREDENTIALS_FILE"),
    };
    let error = open_pair(&selection).err().unwrap();
    assert_eq!(error.class(), crate::ErrorClass::CredentialsFile);
    assert!(error.has_identity(crate::ErrorClass::InvalidConfig));
    let detail = error.detail::<FileDiagnostic>().unwrap();
    assert_eq!(detail.source(), "VNGCLOUD_SHARED_CREDENTIALS_FILE");
    assert!(detail.reason().contains("chmod 600"));
    assert!(
        !detail
            .reason()
            .contains(&credentials.to_string_lossy().to_string())
    );
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(open_pair(&selection).is_ok());
}

#[cfg(unix)]
#[test]
fn opened_pair_rejects_distinct_symlink_aliases() {
    use crate::config::mode::{Mode, Paths, Selection};
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = Dir::new();
    let target = dir.0.join("target");
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&target, b"[default]\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &config).unwrap();
    symlink(&target, &credentials).unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config),
            credentials: Some(credentials),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    let alias_error = open_pair(&selection).err().unwrap();
    assert_eq!(alias_error.class(), crate::ErrorClass::InvalidConfig);
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

#[test]
fn settings_only_validates_credentials_without_content_open() {
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\n").unwrap();
    std::fs::write(&credentials, "toy-value").unwrap();
    let mut selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config.clone()),
            credentials: Some(credentials.clone()),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(std::fs::File::open(&credentials).is_err());
    }
    assert!(matches!(open_settings(&selection), Ok(Some(_))));
    std::fs::remove_file(&credentials).unwrap();
    std::fs::hard_link(&config, &credentials).unwrap();
    assert!(
        matches!(open_settings(&selection), Err(error) if error.class() == crate::ErrorClass::InvalidConfig)
    );
    std::fs::remove_file(&credentials).unwrap();
    selection.credentials_present = false;
    assert!(
        matches!(open_settings(&selection), Err(error) if error.class() == crate::ErrorClass::CredentialsFile)
    );
    let error = open_settings(&selection).err().unwrap();
    let detail = error.detail::<crate::FileDiagnostic>().unwrap();
    assert_eq!(detail.source(), "option");
    assert_eq!(detail.reason(), "not found");
    std::fs::create_dir(&credentials).unwrap();
    selection.credentials_present = true;
    assert!(
        matches!(open_settings(&selection), Err(error) if error.class() == crate::ErrorClass::CredentialsFile)
    );
    let error = open_settings(&selection).err().unwrap();
    assert_eq!(
        error.detail::<crate::FileDiagnostic>().unwrap().reason(),
        "not a regular file"
    );
}

#[test]
fn settings_only_refuses_credentials_swapped_after_snapshot() {
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\nsecret=toy\n").unwrap();
    std::fs::write(&credentials, "[default]\n").unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config.clone()),
            credentials: Some(credentials.clone()),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    let result = open_settings_with(&selection, || {
        std::fs::remove_file(&credentials).unwrap();
        std::fs::hard_link(&config, &credentials).unwrap();
    });
    assert!(
        matches!(result, Err(error) if error.class() == crate::ErrorClass::InvalidConfig && error.detail::<crate::FileDiagnostic>().is_some_and(|detail| detail.slot() == crate::FileSlot::Credentials && detail.source() == "option" && detail.reason() == "changed during read"))
    );
}

#[test]
fn absent_credentials_cannot_appear_as_config_alias() {
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\nsecret=toy\n").unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config.clone()),
            credentials: Some(credentials.clone()),
            config_named: true,
            credentials_named: false,
        },
        config_present: true,
        credentials_present: false,
        config_source: Some("option"),
        credentials_source: Some("~/.greennode"),
    };
    let opens = std::cell::Cell::new(0);
    let result = open_settings_guarded(
        &selection,
        || {
            std::fs::hard_link(&config, &credentials).unwrap();
        },
        || None,
        || opens.set(opens.get() + 1),
    );
    assert_eq!(opens.get(), 0);
    assert!(
        matches!(result, Err(error) if error.class() == crate::ErrorClass::InvalidConfig && error.detail::<crate::FileDiagnostic>().is_some_and(|detail| detail.reason() == "changed during read"))
    );
}

#[test]
fn cancelled_before_open_never_opens_config() {
    use crate::config::mode::{Mode, Paths, Selection};
    use std::cell::Cell;
    let dir = Dir::new();
    let config = dir.0.join("config");
    std::fs::write(&config, "[default]\n").unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config),
            credentials: None,
            config_named: true,
            credentials_named: false,
        },
        config_present: true,
        credentials_present: false,
        config_source: Some("option"),
        credentials_source: None,
    };
    let (ctx, cancel) = crate::RequestContext::new();
    let opens = Cell::new(0);
    let result = open_settings_guarded(
        &selection,
        || cancel.cancel(),
        || ctx.check(&crate::SystemClock),
        || opens.set(opens.get() + 1),
    );
    assert!(matches!(result, Err(error) if error.class() == crate::ErrorClass::Cancelled));
    assert_eq!(opens.get(), 0);
}

#[test]
fn changed_config_identity_has_safe_diagnostic() {
    use crate::config::mode::{Mode, Paths, Selection};
    let dir = Dir::new();
    let config = dir.0.join("config");
    let credentials = dir.0.join("credentials");
    std::fs::write(&config, "[default]\n").unwrap();
    std::fs::write(&credentials, "[default]\n").unwrap();
    let selection = Selection {
        paths: Paths {
            mode: Mode::Greennode,
            config: Some(config.clone()),
            credentials: Some(credentials.clone()),
            config_named: true,
            credentials_named: true,
        },
        config_present: true,
        credentials_present: true,
        config_source: Some("option"),
        credentials_source: Some("option"),
    };
    let result = open_settings_with(&selection, || {
        std::fs::remove_file(&config).unwrap();
        std::fs::hard_link(&credentials, &config).unwrap();
    });
    assert!(
        matches!(result, Err(error) if error.class() == crate::ErrorClass::InvalidConfig && error.detail::<crate::FileDiagnostic>().is_some_and(|detail| detail.slot() == crate::FileSlot::Config && detail.source() == "option" && detail.reason() == "changed during read"))
    );
}
