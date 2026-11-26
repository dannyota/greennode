use super::{Slot, file, ini, mode, settings};
use crate::{ConfigSource, Error, ErrorClass, FileDiagnostic, FileSlot, Secret};
use std::collections::BTreeMap;
use std::path::Path;

/// Selects the configuration family and paths using the supplied source and working directory.
///
/// This probes file presence without reading contents. On Windows, probes use metadata-only handles.
#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
pub fn select(
    config: Option<&Path>,
    credentials: Option<&Path>,
    source: &dyn ConfigSource,
    cwd: Option<&Path>,
) -> Result<mode::Selection, Error> {
    mode::select_for_configure(config, credentials, source, cwd, |path| {
        mode::probe_checked(path, &|| false)
    })
    .map_err(|error| *error)
}

/// Parsed sections and values from one configuration slot.
///
/// Secret fields retain only an empty/nonempty mask.
#[derive(Default, Debug)]
pub struct ProfileFile {
    sections: BTreeMap<String, BTreeMap<String, Secret>>,
}

impl ProfileFile {
    /// Returns sections keyed by their stored names; secret fields contain only an empty/nonempty mask.
    pub fn sections(&self) -> &BTreeMap<String, BTreeMap<String, Secret>> {
        &self.sections
    }

    /// Reports whether a stored key has a nonempty value.
    pub fn is_set(&self, section: &str, key: &str) -> bool {
        self.sections
            .get(section)
            .and_then(|values| values.get(key))
            .is_some_and(|value| !value.expose_secret().is_empty())
    }

    /// Returns the named profile's validated file latch, rejecting malformed values.
    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Error by value"
    )]
    pub fn read_only(&self, profile: &str) -> Result<bool, Error> {
        let section = if profile == "default" {
            profile.to_owned()
        } else {
            format!("profile {profile}")
        };
        let value = self.sections.get(&section).and_then(|s| s.get("read_only"));
        settings::parse_latch(value.map(Secret::expose_secret), true)
            .map_err(|_| Error::new(ErrorClass::InvalidConfig))
    }
}

/// Reads one selected profile from a slot using the existing file safety and parsing rules.
///
/// This is a blocking operation. A missing unnamed file returns empty sections.
#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
pub fn read_slot(
    selection: &mode::Selection,
    slot: FileSlot,
    profile: &str,
) -> Result<ProfileFile, Error> {
    for (index, failed_slot, source) in [
        (0, FileSlot::Config, selection.config_source),
        (1, FileSlot::Credentials, selection.credentials_source),
    ] {
        if selection.lookup_failed[index] {
            let class = if failed_slot == FileSlot::Credentials && selection.credentials_named {
                ErrorClass::CredentialsFile
            } else {
                ErrorClass::InvalidConfig
            };
            return Err(Error::new(class).with_detail(FileDiagnostic::from_text(
                failed_slot,
                source,
                "file: lookup failed",
            )));
        }
    }
    if slot == FileSlot::Credentials
        && selection.credentials_named
        && !selection.credentials_present
    {
        return Err(Error::new(ErrorClass::CredentialsFile).with_detail(
            FileDiagnostic::from_text(slot, selection.credentials_source, "credentials: not found"),
        ));
    }
    let (file, source) = match slot {
        FileSlot::Config => (
            file::open_settings_checked(selection, || None).map_err(|error| *error)?,
            selection.config_source,
        ),
        FileSlot::Credentials => (
            file::open_pair_checked(selection, || None)
                .map_err(|error| *error)?
                .credentials,
            selection.credentials_source,
        ),
    };
    let Some(file) = file else {
        return Ok(ProfileFile::default());
    };
    let error = |diagnostic: String| {
        let class = match slot {
            FileSlot::Config => ErrorClass::InvalidConfig,
            FileSlot::Credentials => ErrorClass::CredentialsFile,
        };
        Error::new(class).with_detail(FileDiagnostic::from_text(slot, source, &diagnostic))
    };
    let parse_slot = match slot {
        FileSlot::Config => Slot::Config,
        FileSlot::Credentials => Slot::Credentials,
    };
    let mut sections = ini::parse_profile_checked(parse_slot, file, &|| false)
        .map_err(error)?
        .0;
    let section = if slot == FileSlot::Config && profile != "default" {
        format!("profile {profile}")
    } else {
        profile.to_owned()
    };
    sections.retain(|name, _| name == &section);
    if slot == FileSlot::Config
        && sections.values().any(|values| {
            ["client_id", "client_secret"].iter().any(|key| {
                values
                    .get(*key)
                    .is_some_and(|value| !value.expose_secret().is_empty())
            })
        })
    {
        return Err(Error::new(ErrorClass::InvalidConfig));
    }
    for values in sections.values_mut() {
        for key in ["password", "totp_secret", "vcdn_api_key", "client_secret"] {
            if let Some(value) = values.get_mut(key) {
                *value = Secret::new(if value.expose_secret().is_empty() {
                    ""
                } else {
                    "1"
                });
            }
        }
    }
    Ok(ProfileFile { sections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    struct Source(PathBuf);
    impl ConfigSource for Source {
        fn var_os(&self, _: &OsStr) -> Option<OsString> {
            None
        }
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    #[cfg(unix)]
    #[test]
    fn selection_probes_fifo_without_opening_in_both_modes() {
        for (directory, expected) in [
            (".greennode", mode::Mode::Greennode),
            (".vngcloud", mode::Mode::Legacy),
        ] {
            let root =
                env::temp_dir().join(format!("select-fifo-{}-{directory}", std::process::id()));
            std::fs::create_dir_all(root.join(directory)).unwrap();
            let path = root.join(directory).join("config");
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            let source = Source(root.clone());
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = send.send(select(None, None, &source, None));
            });
            let selection = receive
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(selection.mode, expected);
            assert!(selection.present(FileSlot::Config));
            assert!(!selection.present(FileSlot::Credentials));
            assert_eq!(selection.config.as_ref(), Some(&path));
            assert_eq!(
                selection.credentials.as_ref(),
                Some(&root.join(directory).join("credentials"))
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn missing_legacy_credentials_keeps_mode_before_read() {
        struct Legacy(PathBuf);
        impl ConfigSource for Legacy {
            fn var_os(&self, name: &OsStr) -> Option<OsString> {
                (name == OsStr::new("VNGCLOUD_SHARED_CREDENTIALS_FILE"))
                    .then(|| self.0.join("missing").into_os_string())
            }
            fn home_dir(&self) -> Option<PathBuf> {
                Some(self.0.clone())
            }
        }
        let root = env::temp_dir().join(format!("select-legacy-missing-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let selected = select(None, None, &Legacy(root.clone()), None).unwrap();
        assert_eq!(selected.mode, mode::Mode::Legacy);
        assert!(!selected.present(FileSlot::Credentials));
        assert_eq!(
            read_slot(&selected, FileSlot::Credentials, "default")
                .unwrap_err()
                .class(),
            ErrorClass::CredentialsFile
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_legacy_override_defers_lookup_error() {
        struct Legacy(PathBuf);
        impl ConfigSource for Legacy {
            fn var_os(&self, name: &OsStr) -> Option<OsString> {
                (name == OsStr::new("VNGCLOUD_SHARED_CREDENTIALS_FILE")).then(|| {
                    self.0
                        .join("not-directory")
                        .join("credentials")
                        .into_os_string()
                })
            }
            fn home_dir(&self) -> Option<PathBuf> {
                Some(self.0.clone())
            }
        }
        use std::os::unix::fs::PermissionsExt;
        let root = env::temp_dir().join(format!("select-legacy-blocked-{}", std::process::id()));
        let blocked = root.join("not-directory");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if mode::probe_checked(&blocked.join("credentials"), &|| false) != mode::Presence::Ambiguous
        {
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::remove_dir(&blocked).unwrap();
            std::fs::write(&blocked, "").unwrap();
        }
        let selected = select(None, None, &Legacy(root.clone()), None).unwrap();
        assert_eq!(selected.mode, mode::Mode::Legacy);
        let error = read_slot(&selected, FileSlot::Credentials, "default").unwrap_err();
        assert_eq!(error.class(), ErrorClass::CredentialsFile);
        assert_eq!(
            error.detail::<FileDiagnostic>().unwrap().reason(),
            "lookup failed"
        );
        if blocked.is_dir() {
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_refuses_reserved_credentials_only_in_selected_profile() {
        let root = env::temp_dir().join(format!("select-reserved-{}", std::process::id()));
        let dir = root.join(".greennode");
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        std::fs::write(
            &config,
            "[default]\nregion=one\n[profile dev]\nclient_id=toy\n",
        )
        .unwrap();
        let selected = select(None, None, &Source(root.clone()), None).unwrap();
        let parsed = read_slot(&selected, FileSlot::Config, "default").unwrap();
        assert_eq!(parsed.sections().len(), 1);
        assert_eq!(
            parsed.sections()["default"]["region"].expose_secret(),
            "one"
        );
        assert_eq!(
            read_slot(&selected, FileSlot::Config, "dev")
                .unwrap_err()
                .class(),
            ErrorClass::InvalidConfig
        );
        std::fs::write(&config, "[profile dev]\nclient_id=\nclient_secret=toy\n").unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Config, "dev")
                .unwrap_err()
                .class(),
            ErrorClass::InvalidConfig
        );
        std::fs::write(&config, "[profile dev]\nclient_id=\nclient_secret=\n").unwrap();
        assert!(read_slot(&selected, FileSlot::Config, "dev").is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reader_enforces_alias_permissions_and_parses_latch() {
        use std::os::unix::fs::PermissionsExt;
        let root = env::temp_dir().join(format!("select-reader-{}", std::process::id()));
        let dir = root.join(".greennode");
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let credentials = dir.join("credentials");
        std::fs::write(
            &config,
            "[default]\nread_only=true\nusername=visible\n[profile dev]\nread_only=0\n",
        )
        .unwrap();
        std::fs::hard_link(&config, &credentials).unwrap();
        let source = Source(root.clone());
        let selected = select(None, None, &source, None).unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Config, "default")
                .unwrap_err()
                .class(),
            ErrorClass::InvalidConfig
        );
        std::fs::remove_file(&credentials).unwrap();
        std::fs::write(&credentials, "[default]\npassword=toy-canary\ntotp_secret=toy-canary\nvcdn_api_key=toy-canary\nclient_secret=toy-canary\nusername=visible\n").unwrap();
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Credentials, "default")
                .unwrap_err()
                .class(),
            ErrorClass::CredentialsFile
        );
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
        let parsed = read_slot(&selected, FileSlot::Credentials, "default").unwrap();
        for key in ["password", "totp_secret", "vcdn_api_key", "client_secret"] {
            assert!(parsed.is_set("default", key));
            assert!(
                !parsed.sections()["default"][key]
                    .expose_secret()
                    .contains("toy-canary")
            );
        }
        assert!(!parsed.is_set("default", "missing"));
        assert_eq!(
            parsed.sections()["default"]["username"].expose_secret(),
            "visible"
        );
        assert!(!format!("{parsed:?}").contains("toy-canary"));
        let parsed = read_slot(&selected, FileSlot::Config, "default").unwrap();
        assert_eq!(
            parsed.sections()["default"]["username"].expose_secret(),
            "visible"
        );
        assert!(parsed.read_only("default").unwrap());
        assert!(!parsed.read_only("dev").unwrap());
        std::fs::write(&config, "[default]\nread_only=maybe\n").unwrap();
        let parsed = read_slot(&selected, FileSlot::Config, "default").unwrap();
        assert_eq!(
            parsed.read_only("default").unwrap_err().class(),
            ErrorClass::InvalidConfig
        );
        std::fs::write(&config, "invalid\n").unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Config, "default")
                .unwrap_err()
                .class(),
            ErrorClass::InvalidConfig
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
