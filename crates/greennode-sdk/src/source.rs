use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub trait ConfigSource: Send + Sync {
    fn var_os(&self, name: &OsStr) -> Option<OsString>;
    fn home_dir(&self) -> Option<PathBuf>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessConfigSource;

impl ConfigSource for ProcessConfigSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        std::env::home_dir()
    }
}
