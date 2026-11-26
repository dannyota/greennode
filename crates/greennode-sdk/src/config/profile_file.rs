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
    mode::select(config, credentials, source, cwd, |path| {
        mode::probe_checked(path, &|| false)
    })
    .map_err(|error| *error)
}

/// Parsed sections and values from one configuration slot.
///
/// Credential fields in a config file retain the existing empty/nonempty mask.
#[derive(Default, Debug)]
pub struct ProfileFile {
    sections: BTreeMap<String, BTreeMap<String, Secret>>,
}

impl ProfileFile {
    /// Returns sections keyed by their stored names, with values protected as secrets.
    pub fn sections(&self) -> &BTreeMap<String, BTreeMap<String, Secret>> {
        &self.sections
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

/// Reads one selected slot using the existing file safety and parsing rules.
///
/// This is a blocking operation. A missing unnamed file returns empty sections.
#[expect(
    clippy::result_large_err,
    reason = "The public API returns Error by value"
)]
pub fn read_slot(selection: &mode::Selection, slot: FileSlot) -> Result<ProfileFile, Error> {
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
    let sections = match slot {
        FileSlot::Config => ini::parse_checked(Slot::Config, file, &|| false)
            .map_err(error)?
            .iter()
            .map(|(section, values)| {
                (
                    section.clone(),
                    values
                        .iter()
                        .map(|(key, value)| (key.clone(), Secret::new(value.clone())))
                        .collect(),
                )
            })
            .collect(),
        FileSlot::Credentials => {
            ini::parse_secrets_checked(file, &|| false)
                .map_err(error)?
                .0
        }
    };
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
            "[default]\nread_only=true\n[profile dev]\nread_only=0\n",
        )
        .unwrap();
        std::fs::hard_link(&config, &credentials).unwrap();
        let source = Source(root.clone());
        let selected = select(None, None, &source, None).unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Config).unwrap_err().class(),
            ErrorClass::InvalidConfig
        );
        std::fs::remove_file(&credentials).unwrap();
        std::fs::write(&credentials, "[default]\npassword=toy\n").unwrap();
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Credentials)
                .unwrap_err()
                .class(),
            ErrorClass::CredentialsFile
        );
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
        let parsed = read_slot(&selected, FileSlot::Credentials).unwrap();
        assert!(parsed.sections()["default"]["password"].expose_secret() == "toy");
        assert!(!format!("{parsed:?}").contains("toy"));
        let parsed = read_slot(&selected, FileSlot::Config).unwrap();
        assert!(parsed.read_only("default").unwrap());
        assert!(!parsed.read_only("dev").unwrap());
        std::fs::write(&config, "[default]\nread_only=maybe\n").unwrap();
        let parsed = read_slot(&selected, FileSlot::Config).unwrap();
        assert_eq!(
            parsed.read_only("default").unwrap_err().class(),
            ErrorClass::InvalidConfig
        );
        std::fs::write(&config, "invalid\n").unwrap();
        assert_eq!(
            read_slot(&selected, FileSlot::Config).unwrap_err().class(),
            ErrorClass::InvalidConfig
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
