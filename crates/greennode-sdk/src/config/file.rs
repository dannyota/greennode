use super::Slot;
use std::fs::File;
use std::path::Path;

#[cfg(unix)]
fn open_flags() -> rustix::fs::OFlags {
    let flags =
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC;
    #[cfg(not(any(
        target_os = "espidf",
        target_os = "horizon",
        target_os = "l4re",
        target_os = "redox",
        target_os = "vita"
    )))]
    let flags = flags | rustix::fs::OFlags::NOCTTY;
    flags
}

#[cfg(windows)]
fn open_windows(
    path: &Path,
    open: impl FnOnce(&std::fs::OpenOptions, &Path) -> std::io::Result<File>,
) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION);
    open(&options, path)
}

pub(super) fn open(path: &Path, slot: Slot) -> Result<File, String> {
    let error = |reason| format!("{}: {reason}", slot.label());
    #[cfg(unix)]
    let file = File::from(
        rustix::fs::open(path, open_flags(), rustix::fs::Mode::empty())
            .map_err(|_| error("open failed"))?,
    );
    #[cfg(windows)]
    let file =
        open_windows(path, |options, path| options.open(path)).map_err(|_| error("open failed"))?;
    #[cfg(not(any(unix, windows)))]
    let file = File::open(path).map_err(|_| error("open failed"))?;
    #[cfg(windows)]
    if !winapi_util::file::typ(&file)
        .map_err(|_| error("metadata failed"))?
        .is_disk()
    {
        return Err(error("not a regular file"));
    }
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
