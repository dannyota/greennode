use std::io::{self, Write};
use std::path::Path;

use greennode_sdk::Secret;

use super::dispatch::LocalError;
use super::input::InputError;
use super::op::Field;

pub(crate) const FLAG: Field = Field {
    name: "secret-file",
    go_type: "string",
    required: true,
};

pub(crate) fn check(path: &Path) -> Result<(), InputError> {
    if path.as_os_str().is_empty() {
        return Err(InputError("--secret-file is required"));
    }
    if file_name(path).is_none() {
        return Err(InputError("--secret-file must name a file"));
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(InputError(
                "--secret-file already exists; refusing to overwrite it",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(_) => return Err(InputError("--secret-file cannot be inspected")),
    }
    match std::fs::metadata(parent(path)) {
        Ok(info) if info.is_dir() => Ok(()),
        Ok(_) => Err(InputError("--secret-file parent path is not a directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(InputError("--secret-file parent directory does not exist"))
        }
        Err(_) => Err(InputError(
            "--secret-file parent directory cannot be inspected",
        )),
    }
}

fn file_name(path: &Path) -> Option<&std::ffi::OsStr> {
    let last = *path.as_os_str().as_encoded_bytes().last()?;
    if std::path::is_separator(char::from(last)) {
        return None;
    }
    path.file_name()
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub(crate) fn failure(message: &'static str) -> LocalError {
    LocalError {
        code: "SecretFileFailed",
        message: message.into(),
        exit: 1,
    }
}

pub(crate) fn write(path: &Path, secret: &Secret) -> io::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{AtFlags, Mode, OFlags, fchmod, fstat, open, openat, statat, unlinkat};

        let name = file_name(path).ok_or(io::ErrorKind::InvalidInput)?;
        let dir = open(
            parent(path),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let fd = openat(
            &dir,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let mut file = std::fs::File::from(fd);
        let result = fchmod(&file, Mode::RUSR | Mode::WUSR)
            .map_err(io::Error::from)
            .and_then(|()| write_contents(&mut file, secret));
        if result.is_err()
            && let (Ok(created), Ok(current)) =
                (fstat(&file), statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW))
            && (created.st_dev, created.st_ino) == (current.st_dev, current.st_ino)
        {
            let _ = unlinkat(&dir, name, AtFlags::empty());
        }
        result
    }
    #[cfg(not(unix))]
    {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        let result = write_contents(&mut file, secret);
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

fn write_contents(file: &mut std::fs::File, secret: &Secret) -> io::Result<()> {
    let bytes = secret.expose_secret().as_bytes();
    file.write_all(bytes)?;
    if !bytes.ends_with(b"\n") {
        file.write_all(b"\n")?;
    }
    file.sync_all()
}

#[cfg(test)]
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
        check(&path).unwrap();
        write(&path, &first).unwrap();
        assert!(check(&path).is_err());
        assert!(write(&path, &Secret::new("second-toy-key")).is_err());
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
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(check(&path).is_err());
        assert!(write(&path, &Secret::new("toy-key")).is_err());
        assert!(!target.exists());
    }
}
