use crate::ConfigSource;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Presence {
    Present,
    Absent,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mode {
    Greennode,
    Legacy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfigSlot {
    Config,
    Credentials,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Paths {
    pub mode: Mode,
    pub config: Option<PathBuf>,
    pub credentials: Option<PathBuf>,
    pub config_named: bool,
    pub credentials_named: bool,
}

#[derive(Debug)]
pub(super) struct Selection {
    pub paths: Paths,
    pub config_present: bool,
    pub credentials_present: bool,
    pub config_source: Option<&'static str>,
    pub credentials_source: Option<&'static str>,
}

impl std::ops::Deref for Selection {
    type Target = Paths;

    fn deref(&self) -> &Paths {
        &self.paths
    }
}

#[cfg(any(windows, test))]
fn windows_error_presence(raw: Option<i32>) -> Presence {
    match raw {
        Some(2 | 3) => Presence::Absent,
        _ => Presence::Ambiguous,
    }
}

#[cfg(windows)]
pub(super) fn probe(path: &Path) -> Presence {
    probe_windows_with(path, |options, path| options.open(path))
}

#[cfg(windows)]
fn probe_windows_with(
    path: &Path,
    open: impl FnOnce(&std::fs::OpenOptions, &Path) -> std::io::Result<std::fs::File>,
) -> Presence {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    };
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION);
    match open(&options, path) {
        Ok(file) => {
            if file.metadata().is_ok() && winapi_util::file::typ(&file).is_ok() {
                Presence::Present
            } else {
                Presence::Ambiguous
            }
        }
        Err(error) => windows_error_presence(error.raw_os_error()),
    }
}

#[cfg(not(windows))]
pub(super) fn probe(path: &Path) -> Presence {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Presence::Present,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut parent = path.parent();
            while let Some(path) = parent {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return if std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
                            Presence::Absent
                        } else {
                            Presence::Ambiguous
                        };
                    }
                    Ok(metadata) => {
                        return if metadata.is_dir() {
                            Presence::Absent
                        } else {
                            Presence::Ambiguous
                        };
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        parent = path.parent()
                    }
                    Err(_) => return Presence::Ambiguous,
                }
            }
            Presence::Absent
        }
        Err(_) => Presence::Ambiguous,
    }
}

pub(super) fn select(
    config: Option<&Path>,
    credentials: Option<&Path>,
    source: &dyn ConfigSource,
    cwd: Option<&Path>,
    probe: impl Fn(&Path) -> Presence,
) -> Result<Selection, Box<crate::Error>> {
    let invalid = |slot: ConfigSlot| {
        Box::new(crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(slot))
    };
    let credentials_error = |source, reason: &'static str| {
        Box::new(
            crate::Error::new(crate::ErrorClass::CredentialsFile).with_detail(
                super::credentials::FileDiagnostic::from_text(
                    crate::FileSlot::Credentials,
                    source,
                    &format!("credentials: {reason}"),
                ),
            ),
        )
    };
    let value = |name: &str| {
        source
            .var_os(OsStr::new(name))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let options = [config, credentials].map(|v| {
        v.filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
    });
    let green = [
        value("GREENNODE_CONFIG_FILE"),
        value("GREENNODE_SHARED_CREDENTIALS_FILE"),
    ];
    let legacy = [
        value("VNGCLOUD_CONFIG_FILE"),
        value("VNGCLOUD_SHARED_CREDENTIALS_FILE"),
    ];
    let home = source.home_dir().filter(|path| path.is_absolute());
    let defaults = |directory: &str| {
        ["config", "credentials"].map(|name| home.as_ref().map(|h| h.join(directory).join(name)))
    };
    let gd = defaults(".greennode");
    let vd = defaults(".vngcloud");
    let presence = |path: &Option<PathBuf>| path.as_deref().map(&probe).unwrap_or(Presence::Absent);
    let mut gd_seen = None;
    let mut vd_seen = None;
    let mode = if options.iter().chain(green.iter()).any(Option::is_some) {
        Mode::Greennode
    } else {
        let entries = gd.each_ref().map(&presence);
        gd_seen = Some(entries);
        if entries.contains(&Presence::Present) {
            Mode::Greennode
        } else if entries.contains(&Presence::Ambiguous) {
            return Err(invalid(if entries[0] == Presence::Ambiguous {
                ConfigSlot::Config
            } else {
                ConfigSlot::Credentials
            }));
        } else if legacy.iter().any(Option::is_some) {
            Mode::Legacy
        } else {
            let entries = vd.each_ref().map(&presence);
            vd_seen = Some(entries);
            if entries.contains(&Presence::Present) {
                Mode::Legacy
            } else if entries.contains(&Presence::Ambiguous) {
                return Err(invalid(if entries[0] == Presence::Ambiguous {
                    ConfigSlot::Config
                } else {
                    ConfigSlot::Credentials
                }));
            } else {
                Mode::Greennode
            }
        }
    };
    let mut chosen = [None, None];
    let mut named = [false, false];
    let mut labels = [None, None];
    for i in 0..2 {
        let explicit = if mode == Mode::Greennode {
            options[i].clone().or_else(|| green[i].clone())
        } else {
            legacy[i].clone()
        };
        named[i] = explicit.is_some();
        chosen[i] = explicit.or_else(|| {
            if mode == Mode::Greennode {
                gd[i].clone()
            } else {
                vd[i].clone()
            }
        });
        labels[i] = if chosen[i].is_none() {
            None
        } else if mode == Mode::Greennode && options[i].is_some() {
            Some("option")
        } else if mode == Mode::Greennode && green[i].is_some() {
            Some(["GREENNODE_CONFIG_FILE", "GREENNODE_SHARED_CREDENTIALS_FILE"][i])
        } else if mode == Mode::Legacy && legacy[i].is_some() {
            Some(["VNGCLOUD_CONFIG_FILE", "VNGCLOUD_SHARED_CREDENTIALS_FILE"][i])
        } else if mode == Mode::Greennode {
            Some("~/.greennode")
        } else {
            Some("~/.vngcloud")
        };
        if let Some(path) = &mut chosen[i]
            && !path.is_absolute()
        {
            let slot = if i == 0 {
                ConfigSlot::Config
            } else {
                ConfigSlot::Credentials
            };
            #[cfg(windows)]
            if path.has_root()
                || matches!(
                    path.components().next(),
                    Some(std::path::Component::Prefix(_))
                )
            {
                return Err(invalid(slot));
            }
            let base = cwd
                .filter(|p| p.is_absolute())
                .ok_or_else(|| invalid(slot))?;
            *path = base.join(&*path);
            if !path.is_absolute() {
                return Err(invalid(slot));
            }
        }
    }
    let states = [0, 1].map(|i| {
        let cached = if named[i] {
            None
        } else if mode == Mode::Greennode {
            gd_seen
        } else {
            vd_seen
        };
        cached.map_or_else(|| presence(&chosen[i]), |states| states[i])
    });
    if states[0] == Presence::Ambiguous {
        return Err(invalid(ConfigSlot::Config));
    }
    if states[1] == Presence::Ambiguous {
        return Err(if named[1] {
            credentials_error(labels[1], "lookup failed")
        } else {
            invalid(ConfigSlot::Credentials)
        });
    }
    if mode == Mode::Greennode
        && !states.iter().all(|s| *s == Presence::Present)
        && (legacy.iter().any(Option::is_some)
            || vd.iter().any(|p| presence(p) != Presence::Absent))
    {
        return Err(invalid(if states[0] == Presence::Absent {
            ConfigSlot::Config
        } else {
            ConfigSlot::Credentials
        }));
    }
    if named[1] && states[1] == Presence::Absent {
        return Err(credentials_error(labels[1], "not found"));
    }
    Ok(Selection {
        paths: Paths {
            mode,
            config: chosen[0].clone(),
            credentials: chosen[1].clone(),
            config_named: named[0],
            credentials_named: named[1],
        },
        config_present: states[0] == Presence::Present,
        credentials_present: states[1] == Presence::Present,
        config_source: labels[0],
        credentials_source: labels[1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    #[cfg(unix)]
    use std::env;
    use std::ffi::OsString;

    fn absolute(path: &str) -> PathBuf {
        #[cfg(windows)]
        {
            Path::new(r"C:\").join(path.trim_start_matches('/'))
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(path)
        }
    }

    #[test]
    fn selected_sources_name_both_slots() {
        let config = absolute("/chosen/config");
        let credentials = absolute("/chosen/credentials");
        let mut source = Source::default();
        source.0.insert(
            "GREENNODE_CONFIG_FILE".into(),
            config.clone().into_os_string(),
        );
        source.0.insert(
            "GREENNODE_SHARED_CREDENTIALS_FILE".into(),
            credentials.clone().into_os_string(),
        );
        let vars = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |_| Presence::Present,
        )
        .unwrap();
        assert_eq!(vars.config_source, Some("GREENNODE_CONFIG_FILE"));
        assert_eq!(
            vars.credentials_source,
            Some("GREENNODE_SHARED_CREDENTIALS_FILE")
        );
        let options = select(
            Some(&config),
            Some(&credentials),
            &source,
            Some(absolute("/work").as_path()),
            |_| Presence::Present,
        )
        .unwrap();
        assert_eq!(options.config_source, Some("option"));
        assert_eq!(options.credentials_source, Some("option"));
        let mut source = Source::default();
        source
            .0
            .insert("VNGCLOUD_CONFIG_FILE".into(), config.into_os_string());
        source.0.insert(
            "VNGCLOUD_SHARED_CREDENTIALS_FILE".into(),
            credentials.into_os_string(),
        );
        let legacy = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path.starts_with(absolute("/chosen")) {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert_eq!(legacy.config_source, Some("VNGCLOUD_CONFIG_FILE"));
        assert_eq!(
            legacy.credentials_source,
            Some("VNGCLOUD_SHARED_CREDENTIALS_FILE")
        );
        let legacy_defaults = select(
            None,
            None,
            &Source::default(),
            Some(absolute("/work").as_path()),
            |path| {
                if path.ends_with(".vngcloud/config") {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert_eq!(legacy_defaults.config_source, Some("~/.vngcloud"));
        assert_eq!(legacy_defaults.credentials_source, Some("~/.vngcloud"));
        let defaults = select(
            None,
            None,
            &Source::default(),
            Some(absolute("/work").as_path()),
            |_| Presence::Absent,
        )
        .unwrap();
        assert_eq!(defaults.config_source, Some("~/.greennode"));
        assert_eq!(defaults.credentials_source, Some("~/.greennode"));
    }

    #[test]
    fn shared_paths_are_absolute() {
        assert!(absolute("/home/test").is_absolute());
        assert!(absolute("/work").is_absolute());
    }

    #[derive(Default)]
    struct Source(HashMap<OsString, OsString>);
    impl ConfigSource for Source {
        fn var_os(&self, name: &OsStr) -> Option<OsString> {
            self.0.get(name).cloned()
        }
        fn home_dir(&self) -> Option<PathBuf> {
            Some(absolute("/home/test"))
        }
    }

    #[test]
    fn windows_not_found_codes_are_narrowly_classified() {
        assert_eq!(windows_error_presence(Some(2)), Presence::Absent);
        assert_eq!(windows_error_presence(Some(3)), Presence::Absent);
        assert_eq!(windows_error_presence(Some(53)), Presence::Ambiguous);
        assert_eq!(windows_error_presence(None), Presence::Ambiguous);
    }

    #[test]
    fn ambiguous_explicit_credentials_is_credentials_file_error() {
        let result = select(
            None,
            Some(absolute("/cred").as_path()),
            &Source::default(),
            Some(absolute("/work").as_path()),
            |path| {
                if path == absolute("/cred").as_path() {
                    Presence::Ambiguous
                } else {
                    Presence::Absent
                }
            },
        );
        let error = result.err().unwrap();
        assert_eq!(error.class(), crate::ErrorClass::CredentialsFile);
        assert!(error.has_identity(crate::ErrorClass::InvalidConfig));
        let detail = error
            .detail::<super::super::credentials::FileDiagnostic>()
            .unwrap();
        assert_eq!(detail.source(), "option");
        assert_eq!(detail.reason(), "lookup failed");
        assert!(
            !detail
                .reason()
                .contains(&absolute("/cred").to_string_lossy().to_string())
        );
        let result = select(
            None,
            Some(absolute("/cred").as_path()),
            &Source::default(),
            Some(absolute("/work").as_path()),
            |path| {
                if path == absolute("/cred").as_path() {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn selected_file_presence_is_preserved() {
        let source = Source::default();
        let chosen = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path.ends_with(".greennode/config") {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert!(chosen.config_present);
        assert!(!chosen.credentials_present);
        let absent = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |_| Presence::Absent,
        )
        .unwrap();
        assert!(!absent.config_present);
        assert!(!absent.credentials_present);
        let calls = std::cell::Cell::new(0);
        let observed = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path.ends_with(".greennode/config") {
                    let count = calls.get();
                    calls.set(count + 1);
                    if count == 0 {
                        Presence::Present
                    } else {
                        Presence::Absent
                    }
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert!(observed.config_present);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn named_credential_errors_keep_variable_source_without_path() {
        for variable in [
            "GREENNODE_SHARED_CREDENTIALS_FILE",
            "VNGCLOUD_SHARED_CREDENTIALS_FILE",
        ] {
            for state in [Presence::Absent, Presence::Ambiguous] {
                let path = absolute("/private/credentials-canary");
                let mut source = Source::default();
                source
                    .0
                    .insert(variable.into(), path.clone().into_os_string());
                let result = select(
                    None,
                    None,
                    &source,
                    Some(absolute("/work").as_path()),
                    |candidate| {
                        if candidate == path {
                            state
                        } else {
                            Presence::Absent
                        }
                    },
                );
                let error = result.err().unwrap();
                assert_eq!(error.class(), crate::ErrorClass::CredentialsFile);
                let detail = error
                    .detail::<super::super::credentials::FileDiagnostic>()
                    .unwrap();
                assert_eq!(detail.source(), variable);
                assert!(!format!("{detail:?}").contains("credentials-canary"));
            }
        }
    }

    #[test]
    fn explicitly_named_missing_credentials_refuses() {
        let source = Source::default();
        let missing = select(
            None,
            Some(absolute("/missing").as_path()),
            &source,
            Some(absolute("/work").as_path()),
            |_| Presence::Absent,
        );
        let missing = missing.err().unwrap();
        assert_eq!(missing.class(), crate::ErrorClass::CredentialsFile);
        assert!(missing.has_identity(crate::ErrorClass::InvalidConfig));
        let detail = missing
            .detail::<super::super::credentials::FileDiagnostic>()
            .unwrap();
        assert_eq!(detail.source(), "option");
        assert_eq!(detail.reason(), "not found");
        let present = select(
            None,
            Some(absolute("/present").as_path()),
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path == absolute("/present").as_path() {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(present.is_ok());
    }

    #[test]
    fn mode_error_identifies_the_ambiguous_slot() {
        let source = Source::default();
        let result = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path.ends_with(".greennode/credentials") {
                    Presence::Ambiguous
                } else {
                    Presence::Absent
                }
            },
        );
        let error = result.err().unwrap();
        assert_eq!(error.class(), crate::ErrorClass::InvalidConfig);
        assert_eq!(error.detail::<ConfigSlot>(), Some(&ConfigSlot::Credentials));
    }

    #[test]
    fn path_table_and_incomplete_pairs() {
        let home = absolute("/home/test");
        let gd = [
            home.join(".greennode/config"),
            home.join(".greennode/credentials"),
        ];
        let vd = [
            home.join(".vngcloud/config"),
            home.join(".vngcloud/credentials"),
        ];
        let gn = ["GREENNODE_CONFIG_FILE", "GREENNODE_SHARED_CREDENTIALS_FILE"];
        let vn = ["VNGCLOUD_CONFIG_FILE", "VNGCLOUD_SHARED_CREDENTIALS_FILE"];
        for row in 0..6 {
            for legacy in 0..4 {
                let mut source = Source::default();
                let mut present = HashSet::new();
                let gmask = match row {
                    3 => 1,
                    4 => 2,
                    5 => 3,
                    _ => 0,
                };
                for i in 0..2 {
                    if gmask & (1 << i) != 0 {
                        source.0.insert(gn[i].into(), gd[i].clone().into());
                    }
                    if legacy & (1 << i) != 0 {
                        source.0.insert(vn[i].into(), vd[i].clone().into());
                    }
                }
                if row < 2 && legacy & 2 != 0 {
                    present.insert(vd[1].clone());
                }
                if row == 1 {
                    present.insert(vd[0].clone());
                }
                if row == 2 {
                    present.extend(gd.iter().cloned());
                }
                if gmask != 0 {
                    present.extend(gd.iter().cloned());
                }
                let selected = select(
                    None,
                    None,
                    &source,
                    Some(absolute("/work").as_path()),
                    |p| {
                        if present.contains(p) {
                            Presence::Present
                        } else {
                            Presence::Absent
                        }
                    },
                )
                .unwrap();
                let expected = if row >= 2 || (row == 0 && legacy == 0) {
                    Mode::Greennode
                } else {
                    Mode::Legacy
                };
                assert_eq!(selected.mode, expected, "row {row}, legacy {legacy}");
                assert_eq!(
                    selected.config,
                    Some(if expected == Mode::Greennode {
                        gd[0].clone()
                    } else {
                        vd[0].clone()
                    })
                );
                assert_eq!(
                    selected.credentials,
                    Some(if expected == Mode::Greennode {
                        gd[1].clone()
                    } else {
                        vd[1].clone()
                    })
                );
                assert_eq!(
                    selected.config_named,
                    if expected == Mode::Greennode {
                        gmask & 1 != 0
                    } else {
                        legacy & 1 != 0
                    }
                );
                assert_eq!(
                    selected.credentials_named,
                    if expected == Mode::Greennode {
                        gmask & 2 != 0
                    } else {
                        legacy & 2 != 0
                    }
                );
            }
        }
        let mut overrides = Source::default();
        overrides
            .0
            .insert(gn[0].into(), absolute("/new/config").into_os_string());
        overrides
            .0
            .insert(gn[1].into(), absolute("/new/credentials").into_os_string());
        overrides
            .0
            .insert(vn[0].into(), absolute("/old/config").into_os_string());
        let selected = select(
            None,
            None,
            &overrides,
            Some(absolute("/work").as_path()),
            |p| {
                if p.starts_with(absolute("/new")) {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert_eq!(selected.mode, Mode::Greennode);
        assert_eq!(selected.config, Some(absolute("/new/config")));
        for (new_slot, new_path) in gd.iter().enumerate() {
            for old_slot in 0..2 {
                if new_slot == 0 && old_slot == 1 {
                    continue;
                }
                for variable in [false, true] {
                    let mut source = Source::default();
                    let mut present = HashSet::from([new_path.clone()]);
                    if variable {
                        source
                            .0
                            .insert(vn[old_slot].into(), vd[old_slot].clone().into());
                    } else {
                        present.insert(vd[old_slot].clone());
                    }
                    assert!(
                        select(
                            None,
                            None,
                            &source,
                            Some(absolute("/work").as_path()),
                            |p| {
                                if present.contains(p) {
                                    Presence::Present
                                } else {
                                    Presence::Absent
                                }
                            }
                        )
                        .is_err(),
                        "new {new_slot} old {old_slot} variable {variable}"
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_overrides_are_distinct_from_defaults() {
        let mut source = Source::default();
        source.0.insert(
            "VNGCLOUD_CONFIG_FILE".into(),
            absolute("/overrides/config").into_os_string(),
        );
        source.0.insert(
            "VNGCLOUD_SHARED_CREDENTIALS_FILE".into(),
            absolute("/overrides/credentials").into_os_string(),
        );
        let paths = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |path| {
                if path.starts_with(absolute("/overrides")) {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert_eq!(paths.mode, Mode::Legacy);
        assert_eq!(paths.config, Some(absolute("/overrides/config")));
        assert_eq!(paths.credentials, Some(absolute("/overrides/credentials")));
    }

    #[test]
    fn legacy_credentials_default_alone_selects_legacy() {
        let source = Source::default();
        let result = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |p| {
                if p.ends_with(".vngcloud/credentials") {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(result.is_ok());
        let paths = result.unwrap();
        assert_eq!(paths.mode, Mode::Legacy);
    }

    #[test]
    fn empty_overrides_do_not_select_mode() {
        let mut source = Source::default();
        source.0.insert("GREENNODE_CONFIG_FILE".into(), "".into());
        source.0.insert("VNGCLOUD_CONFIG_FILE".into(), "".into());
        let result = select(
            Some(Path::new("")),
            None,
            &source,
            Some(absolute("/work").as_path()),
            |p| {
                if p.ends_with(".vngcloud/config") {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(result.is_ok());
        let paths = result.unwrap();
        assert_eq!(paths.mode, Mode::Legacy);
        assert!(!paths.config_named);
    }

    #[test]
    fn sdk_option_alone_selects_greennode() {
        let mut source = Source::default();
        source.0.insert(
            "VNGCLOUD_CONFIG_FILE".into(),
            absolute("/old").into_os_string(),
        );
        let paths = select(
            Some(absolute("/override").as_path()),
            Some(absolute("/key").as_path()),
            &source,
            Some(absolute("/work").as_path()),
            |p| {
                if p.starts_with(absolute("/override")) || p.starts_with(absolute("/key")) {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        )
        .unwrap();
        assert_eq!(paths.mode, Mode::Greennode);
    }

    #[test]
    fn ambiguous_legacy_default_guards_incomplete_pair() {
        let source = Source::default();
        let result = select(
            None,
            None,
            &source,
            Some(absolute("/work").as_path()),
            |p| {
                if p.ends_with(".greennode/config") {
                    Presence::Present
                } else if p.ends_with(".vngcloud/credentials") {
                    Presence::Ambiguous
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn fifo_presence_probe_never_blocks() {
        let dir = env::temp_dir().join(format!("greennode-mode-fifo-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(probe(&path));
        });
        let result = receive.recv_timeout(std::time::Duration::from_secs(1));
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(result, Ok(Presence::Present));
    }

    #[cfg(windows)]
    #[test]
    fn windows_probe_uses_identification_and_metadata_only() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
            SECURITY_SQOS_PRESENT,
        };
        let result = probe_windows_with(Path::new(r"C:\missing"), |options, _| {
            let debug = format!("{options:?}");
            assert!(debug.contains("access_mode: Some(0)"));
            assert!(debug.contains(&format!(
                "custom_flags: {}",
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
            )));
            assert!(debug.contains(&format!(
                "security_qos_flags: {}",
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION
            )));
            Err(std::io::Error::from_raw_os_error(2))
        });
        assert_eq!(result, Presence::Absent);
        let denied = probe_windows_with(Path::new(r"C:\denied"), |_, _| {
            Err(std::io::ErrorKind::PermissionDenied.into())
        });
        assert_eq!(denied, Presence::Ambiguous);
        let network = probe_windows_with(Path::new(r"C:\network"), |_, _| {
            Err(std::io::Error::from_raw_os_error(53))
        });
        assert_eq!(network, Presence::Ambiguous);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_leaf_is_present() {
        let dir = env::temp_dir().join(format!("greennode-mode-leaf-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("link")).unwrap();
        assert_eq!(probe(&dir.join("link")), Presence::Present);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn relative_home_is_not_probed() {
        struct RelativeHome;
        impl ConfigSource for RelativeHome {
            fn var_os(&self, name: &OsStr) -> Option<OsString> {
                (name == OsStr::new("VNGCLOUD_CONFIG_FILE"))
                    .then(|| absolute("/legacy/config").into_os_string())
            }
            fn home_dir(&self) -> Option<PathBuf> {
                Some(PathBuf::from("relative-home"))
            }
        }
        let seen = std::cell::RefCell::new(Vec::new());
        let result = select(
            None,
            None,
            &RelativeHome,
            Some(absolute("/work").as_path()),
            |p| {
                seen.borrow_mut().push(p.to_path_buf());
                if p.ends_with(".greennode/config") {
                    Presence::Present
                } else {
                    Presence::Absent
                }
            },
        );
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.mode, Mode::Legacy);
        assert_eq!(result.config, Some(absolute("/legacy/config")));
        assert!(
            seen.borrow()
                .iter()
                .all(|p| !p.starts_with("relative-home"))
        );
    }

    #[test]
    fn relative_override_without_cwd_is_refused() {
        let result = select(
            Some(Path::new("relative")),
            None,
            &Source::default(),
            None,
            |_| Presence::Absent,
        );
        assert!(result.is_err());
        let absolute = select(
            Some(absolute("/absolute").as_path()),
            None,
            &Source::default(),
            None,
            |_| Presence::Absent,
        );
        assert!(absolute.is_ok());
    }

    #[test]
    fn relative_override_is_anchored_before_probing() {
        let seen = std::cell::RefCell::new(Vec::new());
        let paths = select(
            Some(Path::new("new/config")),
            Some(Path::new("new/credentials")),
            &Source::default(),
            Some(absolute("/work").as_path()),
            |p| {
                seen.borrow_mut().push(p.to_path_buf());
                Presence::Present
            },
        )
        .unwrap();
        assert_eq!(paths.config, Some(absolute("/work/new/config")));
        assert!(seen.borrow().contains(&absolute("/work/new/config")));
        assert!(!seen.borrow().contains(&PathBuf::from("new/config")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_paths_are_refused() {
        for path in [r"C:config", r"\config"] {
            assert!(
                select(
                    Some(Path::new(path)),
                    None,
                    &Source::default(),
                    Some(Path::new(r"C:\work")),
                    |_| Presence::Absent
                )
                .is_err()
            );
        }
    }

    #[test]
    fn options_override_variables_and_resolve_relative_paths() {
        let mut source = Source::default();
        source.0.insert(
            "VNGCLOUD_CONFIG_FILE".into(),
            absolute("/old").into_os_string(),
        );
        source.0.insert(
            "GREENNODE_CONFIG_FILE".into(),
            absolute("/other").into_os_string(),
        );
        let paths = select(
            Some(Path::new("config")),
            Some(Path::new("credentials")),
            &source,
            Some(absolute("/work").as_path()),
            |_| Presence::Present,
        )
        .unwrap();
        assert_eq!(paths.mode, Mode::Greennode);
        assert_eq!(paths.config, Some(absolute("/work/config")));
        assert_eq!(paths.credentials, Some(absolute("/work/credentials")));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_parent_is_ambiguous() {
        let dir = env::temp_dir().join(format!("greennode-mode-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("link")).unwrap();
        assert_eq!(probe(&dir.join("link/config")), Presence::Ambiguous);
        assert_eq!(probe(&dir.join("missing/config")), Presence::Absent);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ambiguous_greennode_default_refuses() {
        let source = Source::default();
        assert!(
            select(
                None,
                None,
                &source,
                Some(absolute("/work").as_path()),
                |p| {
                    if p.ends_with(".greennode/config") {
                        Presence::Ambiguous
                    } else {
                        Presence::Absent
                    }
                }
            )
            .is_err()
        );
        assert!(
            select(
                None,
                None,
                &source,
                Some(absolute("/work").as_path()),
                |_| { Presence::Absent }
            )
            .is_ok()
        );
    }
}
