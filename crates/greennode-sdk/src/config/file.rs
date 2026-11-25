use super::Slot;
use std::fs::File;
use std::path::Path;

pub(super) fn open(path: &Path, slot: Slot) -> Result<File, String> {
    let error = |reason| format!("{}: {reason}", slot.label());
    #[cfg(unix)]
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| error("open failed"))?,
    );
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|_| error("open failed"))?;
    let metadata = file.metadata().map_err(|_| error("metadata failed"))?;
    if !metadata.is_file() {
        return Err(error("not a regular file"));
    }
    if metadata.len() > 1_048_576 {
        return Err(error("file too large"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if matches!(slot, Slot::Credentials) && metadata.permissions().mode() & 0o077 != 0 {
            return Err(error("unsafe permissions; use chmod 600"));
        }
    }
    Ok(file)
}

pub(super) fn same(first: &File, second: &File) -> std::io::Result<bool> {
    Ok(same_file::Handle::from_file(first.try_clone()?)?
        == same_file::Handle::from_file(second.try_clone()?)?)
}

#[cfg(test)]
mod tests;
