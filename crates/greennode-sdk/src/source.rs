use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// Supplies environment-like values for configuration resolution.
pub trait ConfigSource: Send + Sync {
    /// Returns a value without requiring Unicode, or `None` if absent.
    fn var_os(&self, name: &OsStr) -> Option<OsString>;
    /// Uses nonempty `USERPROFILE` on Windows or `HOME` elsewhere; otherwise returns `None`.
    fn home_dir(&self) -> Option<PathBuf> {
        let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        self.var_os(OsStr::new(name))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

/// Reads configuration values from the process environment.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessConfigSource;

impl ConfigSource for ProcessConfigSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        std::env::var_os(name)
    }
}
