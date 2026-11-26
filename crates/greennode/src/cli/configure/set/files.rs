use std::ffi::{OsStr, OsString};
use std::fs::{self, File, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use same_file::Handle;
use zeroize::Zeroizing;

#[cfg(unix)]
use super::FileSlot;
use super::{Error, Result, TargetSlot};

pub(super) const FILE_LIMIT: usize = 1_048_576;

#[derive(PartialEq, Eq)]
pub(super) struct LegacyTree {
    path: PathBuf,
    identity: Option<Handle>,
}

impl LegacyTree {
    pub(super) fn new(home: &Path) -> Result<Self> {
        Self::read(home.join(".vngcloud"))
    }

    fn read(path: PathBuf) -> Result<Self> {
        let identity = match fs::symlink_metadata(&path) {
            Ok(_) => {
                let real = fs::canonicalize(&path).map_err(|_| unsafe_path())?;
                if !real.is_dir() {
                    return Err(unsafe_path());
                }
                let identity = Handle::from_path(real).map_err(|_| unsafe_path())?;
                Some(identity)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return Err(unsafe_path()),
        };
        Ok(Self { path, identity })
    }

    pub(super) fn recheck(&self) -> Result<()> {
        if *self == Self::read(self.path.clone())? {
            return Ok(());
        }
        Err(Error::Config("legacy tree changed during configure set"))
    }

    pub(super) fn check(&self, directory: &Path) -> Result<()> {
        let Some(legacy) = &self.identity else {
            return Ok(());
        };
        for ancestor in directory.ancestors() {
            match Handle::from_path(ancestor) {
                Ok(identity) if identity == *legacy => return Err(legacy_path()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(unsafe_path()),
            }
        }
        Ok(())
    }
}

fn legacy_path() -> Error {
    Error::Config("configure set refuses a path inside the legacy ~/.vngcloud tree")
}

fn unsafe_path() -> Error {
    Error::Config("cannot verify that the configuration path is outside the legacy tree")
}

pub(super) fn resolve(path: &Path, present: bool, slot: TargetSlot) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(real) => Ok(real),
        Err(error) if !present && !slot.named && error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| slot.error("invalid path", None))?;
            let name = path
                .file_name()
                .ok_or_else(|| slot.error("invalid path", None))?;
            let parent = match fs::canonicalize(parent) {
                Ok(real) => real,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let home = parent
                        .parent()
                        .ok_or_else(|| slot.error("invalid path", None))?;
                    let name = parent
                        .file_name()
                        .ok_or_else(|| slot.error("invalid path", None))?;
                    fs::canonicalize(home)
                        .map_err(|_| slot.error("parent lookup failed", None))?
                        .join(name)
                }
                Err(_) => return Err(slot.error("parent lookup failed", None)),
            };
            Ok(parent.join(name))
        }
        Err(error) if !present && error.kind() == io::ErrorKind::NotFound => {
            Err(slot.error("not found", None))
        }
        Err(_) => Err(slot.error("path lookup failed", None)),
    }
}

pub(super) struct Contents {
    pub(super) bytes: Zeroizing<Vec<u8>>,
    identity: Option<Handle>,
}

impl std::fmt::Debug for Contents {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Contents([redacted])")
    }
}

pub(super) struct Target {
    parent: Parent,
    pub(super) slot: TargetSlot,
    name: OsString,
    lock_name: OsString,
    pub(super) lock_path: PathBuf,
}

impl Target {
    pub(super) fn open(path: &Path, slot: TargetSlot, legacy: &LegacyTree) -> Result<Self> {
        let directory = path
            .parent()
            .ok_or_else(|| slot.error("invalid path", None))?;
        let name = path
            .file_name()
            .ok_or_else(|| slot.error("invalid path", None))?
            .to_owned();
        let parent = match Parent::open(directory) {
            Ok(parent) => parent,
            Err(error) if !slot.named && error.kind() == io::ErrorKind::NotFound => {
                let ancestor = directory
                    .parent()
                    .ok_or_else(|| slot.error("invalid parent", None))?;
                let child = directory
                    .file_name()
                    .ok_or_else(|| slot.error("invalid parent", None))?;
                let ancestor =
                    Parent::open(ancestor).map_err(|_| slot.error("parent lookup failed", None))?;
                ancestor.verify(legacy)?;
                ancestor
                    .mkdir(child)
                    .map_err(|_| slot.error("cannot create private directory", None))?;
                Parent::open(directory)
                    .map_err(|_| slot.error("cannot open private directory", None))?
            }
            Err(_) => return Err(slot.error("parent lookup failed", None)),
        };
        parent.verify(legacy)?;
        let mut lock_name = name.clone();
        lock_name.push(".lock");
        let lock_path = directory.join(&lock_name);
        Ok(Self {
            parent,
            slot,
            name,
            lock_name,
            lock_path,
        })
    }

    pub(super) fn lock(&self, legacy: &LegacyTree) -> Result<Lock<'_>> {
        self.parent.verify(legacy)?;
        let file = self
            .parent
            .open_file(&self.lock_name, Open::Lock)
            .map_err(|_| Error::Config("cannot open configure lock"))?;
        let identity =
            Handle::from_file(file).map_err(|_| Error::Config("cannot identify configure lock"))?;
        let lock = Lock {
            target: self,
            identity,
        };
        lock.verify(legacy)?;
        Ok(lock)
    }

    pub(super) fn read(&self, was_present: bool) -> Result<Contents> {
        let mut input = match self.parent.open_file(&self.name, Open::Read) {
            Ok(file) => file,
            Err(error)
                if !was_present && !self.slot.named && error.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(Contents {
                    bytes: Zeroizing::new(Vec::new()),
                    identity: None,
                });
            }
            Err(_) => return Err(self.slot.error("open failed", None)),
        };
        let info = input
            .metadata()
            .map_err(|_| self.slot.error("metadata failed", None))?;
        #[cfg(unix)]
        if self.slot.slot == FileSlot::Credentials {
            use std::os::unix::fs::PermissionsExt;
            if info.permissions().mode() & 0o077 != 0 {
                return Err(self.slot.error("unsafe permissions; use chmod 600", None));
            }
        }
        if info.len() > FILE_LIMIT as u64 {
            return Err(self.slot.error("file too large", None));
        }
        let mut bytes = Zeroizing::new(vec![0; FILE_LIMIT + 1]);
        let mut used = 0;
        while let Some(rest) = bytes.get_mut(used..).filter(|rest| !rest.is_empty()) {
            match input.read(rest) {
                Ok(0) => break,
                Ok(n) => used += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(self.slot.error("read failed", None)),
            }
        }
        if used > FILE_LIMIT {
            return Err(self.slot.error("file too large", None));
        }
        bytes.truncate(used);
        let identity =
            Handle::from_file(input).map_err(|_| self.slot.error("metadata failed", None))?;
        Ok(Contents {
            bytes,
            identity: Some(identity),
        })
    }

    pub(super) fn verify_original(&self, contents: &Contents) -> Result<()> {
        let stable = match (
            self.parent.open_file(&self.name, Open::Read),
            &contents.identity,
        ) {
            (Ok(file), Some(original)) => {
                Handle::from_file(file).is_ok_and(|current| current == *original)
            }
            (Err(error), None) => error.kind() == io::ErrorKind::NotFound,
            _ => false,
        };
        if stable {
            Ok(())
        } else {
            Err(self.slot.error("file changed during the edit", None))
        }
    }

    pub(super) fn prepare(
        &self,
        legacy: &LegacyTree,
        write: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> Result<Pending<'_>> {
        self.parent.verify(legacy)?;
        let error = |reason| self.slot.error(reason, None);
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| error("cannot generate temporary file name"))?;
        let random = u128::from_ne_bytes(random);
        let name = OsString::from(format!(".greennode-{random:032x}.tmp"));
        let file = self
            .parent
            .open_file(&name, Open::Create)
            .map_err(|_| error("cannot create temporary file"))?;
        let identity = match Handle::from_file(file) {
            Ok(identity) => identity,
            Err(_) => {
                let _ = self.parent.remove(&name);
                return Err(error("cannot identify temporary file"));
            }
        };
        let mut pending = Pending {
            target: self,
            name,
            identity,
            published: false,
        };
        let file = pending.identity.as_file_mut();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| error("cannot set private file permissions"))?;
        }
        write(file)
            .and_then(|()| file.sync_all())
            .map_err(|_| error("cannot write and sync temporary file"))?;
        Ok(pending)
    }
}

pub(super) struct Lock<'a> {
    target: &'a Target,
    pub(super) identity: Handle,
}

impl Lock<'_> {
    pub(super) fn try_acquire(&self) -> Result<bool> {
        match self.identity.as_file().try_lock() {
            Ok(()) => Ok(true),
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(_)) => Err(Error::Config("cannot acquire configure lock")),
        }
    }

    pub(super) fn verify(&self, legacy: &LegacyTree) -> Result<()> {
        self.target.parent.verify(legacy)?;
        if !self
            .target
            .parent
            .matches(&self.target.lock_name, &self.identity)
        {
            return Err(Error::Config("configure lock changed during the edit"));
        }
        Ok(())
    }
}

pub(super) struct Pending<'a> {
    target: &'a Target,
    name: OsString,
    identity: Handle,
    published: bool,
}

impl Pending<'_> {
    pub(super) async fn publish(&mut self, guard: impl Fn() -> Result<()>) -> Result<()> {
        let retries = if cfg!(windows) { 5 } else { 0 };
        for attempt in 0..=retries {
            guard()?;
            if !self.target.parent.matches(&self.name, &self.identity) {
                return Err(self.target.slot.error("temporary file changed", None));
            }
            match self.target.parent.rename(&self.name, &self.target.name) {
                Ok(()) => {
                    self.published = true;
                    return Ok(());
                }
                Err(error)
                    if attempt < retries && matches!(error.raw_os_error(), Some(32 | 33)) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(_) => break,
            }
        }
        Err(self
            .target
            .slot
            .error("cannot publish temporary file", None))
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.published && self.target.parent.matches(&self.name, &self.identity) {
            let _ = self.target.parent.remove(&self.name);
        }
    }
}

#[derive(Clone, Copy)]
enum Open {
    Read,
    Lock,
    Create,
}

struct Parent {
    path: PathBuf,
    identity: Handle,
}

impl Parent {
    fn matches(&self, name: &OsStr, identity: &Handle) -> bool {
        self.open_file(name, Open::Read)
            .and_then(Handle::from_file)
            .is_ok_and(|opened| opened == *identity)
    }

    fn verify(&self, legacy: &LegacyTree) -> Result<()> {
        legacy.recheck()?;
        legacy.check(&self.path)?;
        let current = Self::open(&self.path).map_err(|_| unsafe_path())?;
        if current.identity != self.identity {
            return Err(Error::Config(
                "configuration directory changed during the edit",
            ));
        }
        #[cfg(unix)]
        if let Some(legacy) = &legacy.identity {
            let mut current = Self::identity(
                self.identity
                    .as_file()
                    .try_clone()
                    .map_err(|_| unsafe_path())?,
            )?;
            loop {
                if current == *legacy {
                    return Err(legacy_path());
                }
                let parent = Self::ancestor(&current)?;
                if parent == current {
                    break;
                }
                current = parent;
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Parent {
    const FLAGS: rustix::fs::OFlags = rustix::fs::OFlags::CLOEXEC
        .union(rustix::fs::OFlags::NOFOLLOW)
        .union(rustix::fs::OFlags::NONBLOCK)
        .union(rustix::fs::OFlags::NOCTTY);

    fn open(path: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags, open};
        let handle = open(
            path,
            Self::FLAGS | OFlags::RDONLY | OFlags::DIRECTORY,
            Mode::empty(),
        )?;
        let identity = Handle::from_file(File::from(handle))?;
        Ok(Self {
            path: path.to_owned(),
            identity,
        })
    }

    fn identity(file: File) -> Result<Handle> {
        Handle::from_file(file).map_err(|_| unsafe_path())
    }

    fn ancestor(directory: &Handle) -> Result<Handle> {
        use rustix::fs::{Mode, OFlags, openat};
        let handle = openat(
            directory.as_file(),
            "..",
            Self::FLAGS | OFlags::RDONLY | OFlags::DIRECTORY,
            Mode::empty(),
        )
        .map_err(|_| unsafe_path())?;
        Self::identity(File::from(handle))
    }

    fn mkdir(&self, name: &OsStr) -> io::Result<()> {
        use rustix::fs::{Mode, mkdirat};
        match mkdirat(self.identity.as_file(), name, Mode::RWXU) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn open_file(&self, name: &OsStr, kind: Open) -> io::Result<File> {
        use rustix::fs::{Mode, OFlags, openat};
        let flags = match kind {
            Open::Read => OFlags::RDONLY,
            Open::Lock => OFlags::RDWR | OFlags::CREATE,
            Open::Create => OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        };
        let file = File::from(openat(
            self.identity.as_file(),
            name,
            Self::FLAGS | flags,
            Mode::RUSR | Mode::WUSR,
        )?);
        if !matches!(kind, Open::Create) && !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        Ok(file)
    }

    fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        let directory = self.identity.as_file();
        Ok(rustix::fs::renameat(directory, from, directory, to)?)
    }

    fn remove(&self, name: &OsStr) -> io::Result<()> {
        Ok(rustix::fs::unlinkat(
            self.identity.as_file(),
            name,
            rustix::fs::AtFlags::empty(),
        )?)
    }
}

#[cfg(not(unix))]
impl Parent {
    fn open(path: &Path) -> io::Result<Self> {
        if !fs::symlink_metadata(path)?.is_dir() {
            return Err(io::Error::other("not a directory"));
        }
        Ok(Self {
            path: path.to_owned(),
            identity: Handle::from_path(path)?,
        })
    }

    fn mkdir(&self, name: &OsStr) -> io::Result<()> {
        match fs::create_dir(self.path.join(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn open_file(&self, name: &OsStr, kind: Open) -> io::Result<File> {
        let mut options = fs::OpenOptions::new();
        match kind {
            Open::Read => options.read(true),
            Open::Lock => options.read(true).write(true).create(true),
            Open::Create => options.write(true).create_new(true),
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
            };
            options
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION);
        }
        let file = options.open(self.path.join(name))?;
        #[cfg(windows)]
        if !matches!(kind, Open::Create) && !winapi_util::file::typ(&file)?.is_disk() {
            return Err(io::Error::other("not a regular file"));
        }
        if !matches!(kind, Open::Create) && !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        Ok(file)
    }

    fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        fs::rename(self.path.join(from), self.path.join(to))
    }

    fn remove(&self, name: &OsStr) -> io::Result<()> {
        fs::remove_file(self.path.join(name))
    }
}
