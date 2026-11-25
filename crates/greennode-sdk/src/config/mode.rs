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

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Paths {
    pub mode: Mode,
    pub config: Option<PathBuf>,
    pub credentials: Option<PathBuf>,
    pub config_named: bool,
    pub credentials_named: bool,
}

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
    cwd: &Path,
    probe: impl Fn(&Path) -> Presence,
) -> Result<Paths, ()> {
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
    let home = source.home_dir();
    let defaults = |directory: &str| {
        ["config", "credentials"].map(|name| home.as_ref().map(|h| h.join(directory).join(name)))
    };
    let gd = defaults(".greennode");
    let vd = defaults(".vngcloud");
    let presence = |path: &Option<PathBuf>| path.as_deref().map(&probe).unwrap_or(Presence::Absent);
    let mode = if options.iter().chain(green.iter()).any(Option::is_some) {
        Mode::Greennode
    } else {
        let entries = gd.each_ref().map(&presence);
        if entries.contains(&Presence::Present) {
            Mode::Greennode
        } else if entries.contains(&Presence::Ambiguous) {
            return Err(());
        } else if legacy.iter().any(Option::is_some) {
            Mode::Legacy
        } else {
            let entries = vd.each_ref().map(&presence);
            if entries.contains(&Presence::Present) {
                Mode::Legacy
            } else if entries.contains(&Presence::Ambiguous) {
                return Err(());
            } else {
                Mode::Greennode
            }
        }
    };
    let mut chosen = [None, None];
    let mut named = [false, false];
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
        if let Some(path) = &mut chosen[i]
            && path.is_relative()
        {
            *path = cwd.join(&*path);
        }
    }
    if mode == Mode::Greennode {
        let states = chosen.each_ref().map(&presence);
        if states.contains(&Presence::Ambiguous) {
            return Err(());
        }
        if !states.iter().all(|s| *s == Presence::Present)
            && (legacy.iter().any(Option::is_some)
                || vd.iter().any(|p| presence(p) != Presence::Absent))
        {
            return Err(());
        }
    }
    Ok(Paths {
        mode,
        config: chosen[0].clone(),
        credentials: chosen[1].clone(),
        config_named: named[0],
        credentials_named: named[1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::env;
    use std::ffi::OsString;

    #[derive(Default)]
    struct Source(HashMap<OsString, OsString>);
    impl ConfigSource for Source {
        fn var_os(&self, name: &OsStr) -> Option<OsString> {
            self.0.get(name).cloned()
        }
        fn home_dir(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/test"))
        }
    }

    #[test]
    fn path_table_and_incomplete_pairs() {
        let home = Path::new("/home/test");
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
                if row == 1 {
                    present.insert(vd[0].clone());
                }
                if row == 2 {
                    present.extend(gd.iter().cloned());
                }
                if gmask != 0 {
                    present.extend(gd.iter().cloned());
                }
                let selected = select(None, None, &source, Path::new("/work"), |p| {
                    if present.contains(p) {
                        Presence::Present
                    } else {
                        Presence::Absent
                    }
                })
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
        overrides.0.insert(gn[0].into(), "/new/config".into());
        overrides.0.insert(gn[1].into(), "/new/credentials".into());
        overrides.0.insert(vn[0].into(), "/old/config".into());
        let selected = select(None, None, &overrides, Path::new("/work"), |p| {
            if p.starts_with("/new") {
                Presence::Present
            } else {
                Presence::Absent
            }
        })
        .unwrap();
        assert_eq!(selected.mode, Mode::Greennode);
        assert_eq!(selected.config, Some(PathBuf::from("/new/config")));
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
                        select(None, None, &source, Path::new("/work"), |p| {
                            if present.contains(p) {
                                Presence::Present
                            } else {
                                Presence::Absent
                            }
                        })
                        .is_err(),
                        "new {new_slot} old {old_slot} variable {variable}"
                    );
                }
            }
        }
    }

    #[test]
    fn options_override_variables_and_resolve_relative_paths() {
        let mut source = Source::default();
        source
            .0
            .insert("VNGCLOUD_CONFIG_FILE".into(), "/old".into());
        source
            .0
            .insert("GREENNODE_CONFIG_FILE".into(), "/other".into());
        let paths = select(
            Some(Path::new("config")),
            Some(Path::new("credentials")),
            &source,
            Path::new("/work"),
            |_| Presence::Present,
        )
        .unwrap();
        assert_eq!(paths.mode, Mode::Greennode);
        assert_eq!(paths.config, Some(PathBuf::from("/work/config")));
        assert_eq!(paths.credentials, Some(PathBuf::from("/work/credentials")));
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
            select(None, None, &source, Path::new("/work"), |p| {
                if p.ends_with(".greennode/config") {
                    Presence::Ambiguous
                } else {
                    Presence::Absent
                }
            })
            .is_err()
        );
        assert!(
            select(None, None, &source, Path::new("/work"), |_| {
                Presence::Absent
            })
            .is_ok()
        );
    }
}
