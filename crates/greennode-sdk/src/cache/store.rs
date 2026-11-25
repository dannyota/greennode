use std::fmt;
use std::path::Path;

#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use zeroize::Zeroizing;

#[cfg(unix)]
const LIMIT: usize = 64 * 1024;

#[allow(dead_code)]
pub(crate) struct Store {
    #[cfg(unix)]
    directory: File,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Store([redacted])")
    }
}

#[allow(dead_code)]
impl Store {
    #[cfg(unix)]
    pub(crate) fn open(path: &Path) -> Result<Self, Box<crate::Error>> {
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| unavailable())?;
        let directory = File::from(fd);
        let metadata = directory.metadata().map_err(|_| unavailable())?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(unavailable());
        }
        Ok(Self { directory })
    }

    #[cfg(not(unix))]
    pub(crate) fn open(_path: &Path) -> Result<Self, Box<crate::Error>> {
        Err(unavailable())
    }

    #[cfg(unix)]
    pub(crate) fn read(&self, name: &str) -> Result<Option<crate::Secret>, Box<crate::Error>> {
        validate_name(name)?;
        let fd = match rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        let file = File::from(fd);
        validate_file(&file)?;
        let mut data = Zeroizing::new(Vec::with_capacity(LIMIT + 2));
        file.take((LIMIT + 1) as u64)
            .read_to_end(&mut data)
            .map_err(|_| unavailable())?;
        if data.len() > LIMIT {
            return Err(unavailable());
        }
        let token = std::str::from_utf8(&data).map_err(|_| unavailable())?;
        Ok(Some(crate::Secret::new(token)))
    }

    #[cfg(not(unix))]
    pub(crate) fn read(&self, _name: &str) -> Result<Option<crate::Secret>, Box<crate::Error>> {
        Err(unavailable())
    }

    #[cfg(unix)]
    pub(crate) fn write(&self, name: &str, data: &crate::Secret) -> Result<(), Box<crate::Error>> {
        validate_name(name)?;
        if data.expose_secret().len() > LIMIT {
            return Err(unavailable());
        }
        match rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => validate_file(&File::from(fd))?,
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(unavailable()),
        }
        let mut random = [0u8; 16];
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut random)
            .map_err(|_| unavailable())?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let temporary = format!(".tok-{suffix}");
        let fd = rustix::fs::openat(
            &self.directory,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| unavailable())?;
        let mut cleanup = Temporary {
            directory: &self.directory,
            name: &temporary,
            committed: false,
        };
        let mut file = File::from(fd);
        file.write_all(data.expose_secret().as_bytes())
            .map_err(|_| unavailable())?;
        file.sync_all().map_err(|_| unavailable())?;
        rustix::fs::renameat(&self.directory, temporary.as_str(), &self.directory, name)
            .map_err(|_| unavailable())?;
        cleanup.committed = true;
        Ok(())
    }

    #[cfg(not(unix))]
    pub(crate) fn write(
        &self,
        _name: &str,
        _data: &crate::Secret,
    ) -> Result<(), Box<crate::Error>> {
        Err(unavailable())
    }
}

#[cfg(unix)]
struct Temporary<'a> {
    directory: &'a File,
    name: &'a str,
    committed: bool,
}

#[cfg(unix)]
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = rustix::fs::unlinkat(self.directory, self.name, AtFlags::empty());
        }
    }
}

#[cfg(unix)]
fn validate_name(name: &str) -> Result<(), Box<crate::Error>> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(unavailable());
    }
    Ok(())
}

#[cfg(unix)]
fn validate_file(file: &File) -> Result<(), Box<crate::Error>> {
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > LIMIT as u64
    {
        return Err(unavailable());
    }
    Ok(())
}

fn unavailable() -> Box<crate::Error> {
    Box::new(crate::Error::new(crate::ErrorClass::TokenCache))
}

#[cfg(test)]
mod tests;
