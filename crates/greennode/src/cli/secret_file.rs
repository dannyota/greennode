use std::io;
use std::path::Path;

use greennode_sdk::Secret;

use super::dispatch::LocalError;
use super::input::InputError;
use super::op::Field;

#[cfg(unix)]
use rustix::fs::{
    AtFlags, Mode, OFlags, fchmod, fstat, linkat, mkdirat, open, openat, statat, unlinkat,
};
#[cfg(unix)]
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::Write,
    os::fd::OwnedFd,
};

pub(crate) const FLAG: Field = Field {
    name: "secret-file",
    go_type: "string",
    required: true,
};

pub(crate) struct Destination {
    #[cfg(unix)]
    parent: OwnedFd,
    #[cfg(unix)]
    name: OsString,
}

impl Destination {
    pub(crate) fn check(path: &Path) -> Result<Self, InputError> {
        if path.as_os_str().is_empty() {
            return Err(InputError("--secret-file is required"));
        }
        #[cfg(unix)]
        {
            let name = file_name(path).ok_or(InputError("--secret-file must name a file"))?;
            let parent_path = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let parent = open(
                parent_path,
                search_flags() | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| match error {
                rustix::io::Errno::NOENT => {
                    InputError("--secret-file parent directory does not exist")
                }
                rustix::io::Errno::NOTDIR => {
                    InputError("--secret-file parent path is not a directory")
                }
                _ => InputError("--secret-file parent directory cannot be inspected"),
            })?;
            match statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Err(InputError(
                    "--secret-file already exists; refusing to overwrite it",
                )),
                Err(rustix::io::Errno::NOENT) => Ok(Self {
                    parent,
                    name: name.to_owned(),
                }),
                Err(_) => Err(InputError("--secret-file cannot be inspected")),
            }
        }
        #[cfg(not(unix))]
        Err(InputError(
            "--secret-file is not supported on this platform",
        ))
    }

    pub(crate) fn write(&self, secret: &Secret) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.publish(|file| {
                let bytes = secret.expose_secret().as_bytes();
                file.write_all(bytes)?;
                if !bytes.ends_with(b"\n") {
                    file.write_all(b"\n")?;
                }
                Ok(())
            })
        }
        #[cfg(not(unix))]
        {
            let _ = secret;
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    #[cfg(unix)]
    fn publish(&self, contents: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
        let mut stage = Stage::new(&self.parent)?;
        let mut file = stage.create_file()?;
        contents(&mut file)?;
        file.sync_all()?;
        linkat(
            &stage.dir,
            Stage::FILE,
            &self.parent,
            &self.name,
            AtFlags::empty(),
        )?;
        Ok(())
    }
}

#[cfg(unix)]
fn file_name(path: &Path) -> Option<&OsStr> {
    let suffix = path
        .as_os_str()
        .as_encoded_bytes()
        .rsplit(|byte| *byte == b'/')
        .next()?;
    if matches!(suffix, b"" | b"." | b"..") {
        return None;
    }
    path.file_name()
}

#[cfg(unix)]
fn search_flags() -> OFlags {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        OFlags::PATH
    }
    #[cfg(target_os = "macos")]
    {
        const O_EXEC: u32 = 0x4000_0000;
        OFlags::from_bits_retain(O_EXEC)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        OFlags::RDONLY | OFlags::NOFOLLOW
    }
}

#[cfg(unix)]
struct Stage<'a> {
    parent: &'a OwnedFd,
    name: String,
    dir: OwnedFd,
    file_created: bool,
}

#[cfg(unix)]
impl<'a> Stage<'a> {
    const FILE: &str = "secret";

    fn create_file(&mut self) -> io::Result<File> {
        let mode = Mode::RUSR | Mode::WUSR;
        let fd = openat(
            &self.dir,
            Self::FILE,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode,
        )?;
        self.file_created = true;
        fchmod(&fd, mode)?;
        Ok(File::from(fd))
    }

    fn new(parent: &'a OwnedFd) -> io::Result<Self> {
        let nonce = getrandom::u64().map_err(|_| io::ErrorKind::Other)?;
        let name = format!(".greennode-secret-{nonce:016x}");
        mkdirat(parent, &name, Mode::RUSR | Mode::WUSR | Mode::XUSR)?;
        let dir = openat(
            parent,
            &name,
            search_flags() | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let stat = fstat(&dir)?;
        if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o077 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(Self {
            parent,
            name,
            dir,
            file_created: false,
        })
    }
}

#[cfg(unix)]
impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if self.file_created {
            let _ = unlinkat(&self.dir, Self::FILE, AtFlags::empty());
        }
        let _ = unlinkat(self.parent, &self.name, AtFlags::REMOVEDIR);
    }
}

pub(crate) fn failure(message: &'static str) -> LocalError {
    LocalError {
        code: "SecretFileFailed",
        message: message.into(),
        exit: 1,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let suffix = getrandom::u64().unwrap();
            let path = std::env::temp_dir().join(format!("greennode-secret-file-{suffix:016x}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn creates_private_file_and_refuses_a_racing_destination() {
        let dir = TempDir::new();
        let path = dir.0.join("secret");
        let first = Secret::new("first-toy-key");
        let file = Destination::check(&path).unwrap();
        file.write(&first).unwrap();
        assert!(Destination::check(&path).is_err());
        assert!(file.write(&Secret::new("second-toy-key")).is_err());
        let contents = std::fs::read(&path).unwrap();
        assert!(contents.strip_suffix(b"\n") == Some(first.expose_secret().as_bytes()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_dangling_symlink() {
        let dir = TempDir::new();
        let path = dir.0.join("secret");
        let target = dir.0.join("missing");
        let file = Destination::check(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(Destination::check(&path).is_err());
        assert!(file.write(&Secret::new("toy-key")).is_err());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn retains_preflight_parent_after_retarget() {
        let dir = TempDir::new();
        let initial = dir.0.join("initial");
        let replacement = dir.0.join("replacement");
        let alias = dir.0.join("alias");
        std::fs::create_dir(&initial).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        std::os::unix::fs::symlink(&initial, &alias).unwrap();
        let path = alias.join("secret");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&initial, std::fs::Permissions::from_mode(0o300)).unwrap();
        let file = Destination::check(&path).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&replacement, &alias).unwrap();
        let saved = file.write(&Secret::new("toy-key"));
        std::fs::set_permissions(&initial, std::fs::Permissions::from_mode(0o700)).unwrap();
        saved.unwrap();
        assert!(initial.join("secret").is_file());
        assert!(!replacement.join("secret").exists());
    }

    #[cfg(unix)]
    #[test]
    fn stages_privately_and_preserves_a_racing_destination() {
        let dir = TempDir::new();
        let path = dir.0.join("secret");
        let destination = Destination::check(&path).unwrap();
        let interrupted = destination.publish(|file| {
            file.write_all(b"toy-key")?;
            Err(io::ErrorKind::Interrupted.into())
        });
        assert!(interrupted.is_err());
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);

        let result = destination.publish(|file| {
            file.write_all(b"toy-key")?;
            assert!(!path.exists());
            std::fs::write(&path, b"racing file")?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(std::fs::read(&path).unwrap() == b"racing file");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_exclusive_stage_create_preserves_the_preexisting_file() {
        let dir = TempDir::new();
        let destination = Destination::check(&dir.0.join("output")).unwrap();
        let mut stage = Stage::new(&destination.parent).unwrap();
        let original = dir.0.join(&stage.name).join(Stage::FILE);
        std::fs::write(&original, b"existing file").unwrap();
        let error = stage.create_file().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        drop(stage);
        assert!(std::fs::read(&original).is_ok_and(|bytes| bytes == b"existing file"));
    }

    #[test]
    fn rejects_literal_final_dot_components() {
        let dir = TempDir::new();
        assert_eq!(
            Destination::check(&dir.0.join("missing/.")).err(),
            Some(InputError("--secret-file must name a file"))
        );
        assert_eq!(
            Destination::check(&dir.0.join("missing/..")).err(),
            Some(InputError("--secret-file must name a file"))
        );
    }
}
