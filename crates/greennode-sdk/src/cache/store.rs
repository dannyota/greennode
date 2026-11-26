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

#[cfg(unix)]
pub(super) enum ReadOutcome {
    Missing,
    Oversize,
    InvalidUtf8,
    Data(crate::Secret),
}

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
    pub(crate) fn create(path: &Path) -> Result<Self, Box<crate::Error>> {
        Self::create_avoiding(path, None)
    }

    #[cfg(unix)]
    pub(crate) fn create_avoiding(
        path: &Path,
        home: Option<&Path>,
    ) -> Result<Self, Box<crate::Error>> {
        let legacy = match home {
            Some(home) => match std::fs::metadata(home.join(".vngcloud")) {
                Ok(metadata) if metadata.is_dir() => Some((metadata.dev(), metadata.ino())),
                Ok(_) => return Err(unavailable()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(unavailable()),
            },
            None => None,
        };
        let mut components = path.components();
        if components.next() != Some(std::path::Component::RootDir) {
            return Err(unavailable());
        }
        let mut current = File::from(
            rustix::fs::open(
                "/",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| unavailable())?,
        );
        for component in components {
            let std::path::Component::Normal(name) = component else {
                return Err(unavailable());
            };
            let fd = match rustix::fs::openat(
                &current,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => {
                    match rustix::fs::mkdirat(&current, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(_) => return Err(unavailable()),
                    }
                    rustix::fs::openat(
                        &current,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|_| unavailable())?
                }
                Err(_) => return Err(unavailable()),
            };
            current = File::from(fd);
            let metadata = current.metadata().map_err(|_| unavailable())?;
            if legacy == Some((metadata.dev(), metadata.ino())) {
                return Err(unavailable());
            }
        }
        Self::from_directory(current.into())
    }

    #[cfg(unix)]
    pub(crate) fn create_child(&self, name: &str) -> Result<Self, Box<crate::Error>> {
        validate_name(name)?;
        match rustix::fs::mkdirat(&self.directory, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(_) => return Err(unavailable()),
        }
        Self::from_directory(
            rustix::fs::openat(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| unavailable())?,
        )
    }

    #[cfg(unix)]
    fn from_directory(fd: rustix::fd::OwnedFd) -> Result<Self, Box<crate::Error>> {
        let directory = File::from(fd);
        let metadata = directory.metadata().map_err(|_| unavailable())?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(unavailable());
        }
        Ok(Self { directory })
    }

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
            || metadata.permissions().mode() & 0o077 != 0
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
    pub(super) fn open_lock(&self, name: &str) -> Result<File, Box<crate::Error>> {
        validate_name(name)?;
        let fd = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| unavailable())?;
        let file = File::from(fd);
        validate_owner_file(&file)?;
        Ok(file)
    }

    #[cfg(unix)]
    pub(super) fn verify_identity(&self, name: &str, file: &File) -> Result<(), Box<crate::Error>> {
        if self.identity_matches(name, file)? {
            Ok(())
        } else {
            Err(unavailable())
        }
    }

    #[cfg(unix)]
    pub(super) fn identity_matches(
        &self,
        name: &str,
        file: &File,
    ) -> Result<bool, Box<crate::Error>> {
        validate_owner_file(file)?;
        let fd = match rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(false),
            Err(_) => return Err(unavailable()),
        };
        let named = File::from(fd);
        validate_owner_file(&named)?;
        let first = same_file::Handle::from_file(file.try_clone().map_err(|_| unavailable())?)
            .map_err(|_| unavailable())?;
        let second = same_file::Handle::from_file(named).map_err(|_| unavailable())?;
        Ok(first == second)
    }

    #[cfg(unix)]
    pub(super) fn read_marker(
        &self,
        name: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, Box<crate::Error>> {
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
        validate_owner_file(&file)?;
        let mut data = Zeroizing::new(Vec::with_capacity(33));
        (&file)
            .take(32)
            .read_to_end(&mut data)
            .map_err(|_| unavailable())?;
        self.verify_identity(name, &file)?;
        Ok(Some(data))
    }

    #[cfg(unix)]
    pub(crate) fn read(&self, name: &str) -> Result<Option<crate::Secret>, Box<crate::Error>> {
        match self.read_entry(name)? {
            ReadOutcome::Missing => Ok(None),
            ReadOutcome::Oversize | ReadOutcome::InvalidUtf8 => Err(unavailable()),
            ReadOutcome::Data(data) => Ok(Some(data)),
        }
    }

    #[cfg(unix)]
    pub(super) fn read_entry(&self, name: &str) -> Result<ReadOutcome, Box<crate::Error>> {
        validate_name(name)?;
        let fd = match rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(ReadOutcome::Missing),
            Err(_) => return Err(unavailable()),
        };
        let file = File::from(fd);
        validate_owner_file(&file)?;
        let mut data = Zeroizing::new(Vec::with_capacity(LIMIT + 2));
        (&file)
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut data)
            .map_err(|_| unavailable())?;
        self.verify_identity(name, &file)?;
        if data.len() > LIMIT {
            return Ok(ReadOutcome::Oversize);
        }
        let Ok(token) = std::str::from_utf8(&data) else {
            return Ok(ReadOutcome::InvalidUtf8);
        };
        Ok(ReadOutcome::Data(crate::Secret::new(token)))
    }

    #[cfg(not(unix))]
    pub(crate) fn read(&self, _name: &str) -> Result<Option<crate::Secret>, Box<crate::Error>> {
        Err(unavailable())
    }

    #[cfg(unix)]
    pub(crate) fn write(&self, name: &str, data: &crate::Secret) -> Result<(), Box<crate::Error>> {
        self.write_guarded(name, data, || Ok(()))
    }

    #[cfg(unix)]
    pub(super) fn write_guarded(
        &self,
        name: &str,
        data: &crate::Secret,
        guard: impl Fn() -> Result<(), Box<crate::Error>>,
    ) -> Result<(), Box<crate::Error>> {
        self.write_guarded_hook(name, data, guard, None, None)
    }

    #[cfg(unix)]
    pub(super) fn write_guarded_hook(
        &self,
        name: &str,
        data: &crate::Secret,
        guard: impl Fn() -> Result<(), Box<crate::Error>>,
        hook: Option<&(dyn Fn() + Send + Sync)>,
        live: Option<&std::sync::Mutex<bool>>,
    ) -> Result<(), Box<crate::Error>> {
        guard()?;
        validate_name(name)?;
        if data.expose_secret().len() > LIMIT {
            return Err(unavailable());
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
        if let Some(hook) = hook {
            hook();
        }
        file.sync_all().map_err(|_| unavailable())?;
        let live_guard = live
            .map(|value| value.lock().map_err(|_| unavailable()))
            .transpose()?;
        if live_guard.as_deref() == Some(&false) {
            return Err(unavailable());
        }
        guard()?;
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
fn validate_owner_file(file: &File) -> Result<(), Box<crate::Error>> {
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
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
