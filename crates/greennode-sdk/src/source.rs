use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub trait ConfigSource: Send + Sync {
    fn var_os(&self, name: &OsStr) -> Option<OsString>;
    fn home_dir(&self) -> Option<PathBuf> {
        let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        self.var_os(OsStr::new(name))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessConfigSource;

impl ConfigSource for ProcessConfigSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        std::env::var_os(name)
    }
}
