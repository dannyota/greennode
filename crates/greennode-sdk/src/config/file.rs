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
    let file = open_without_permissions(path, slot)?;
    if matches!(slot, Slot::Credentials) {
        check_credentials_permissions(&file)?;
    }
    Ok(file)
}

fn check_credentials_permissions(file: &File) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = file
            .metadata()
            .map_err(|_| "credentials: metadata failed".to_owned())?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("credentials: unsafe permissions; use chmod 600".to_owned());
        }
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn open_without_permissions(path: &Path, slot: Slot) -> Result<File, String> {
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
    Ok(file)
}

pub(super) fn same(first: &File, second: &File) -> std::io::Result<bool> {
    Ok(same_file::Handle::from_file(first.try_clone()?)?
        == same_file::Handle::from_file(second.try_clone()?)?)
}

#[derive(Default)]
pub(super) struct Pair {
    pub config: Option<File>,
    pub credentials: Option<File>,
}

pub(super) fn open_pair(selection: &super::mode::Selection) -> Result<Pair, Box<crate::Error>> {
    use super::mode::ConfigSlot;
    let error = |slot| {
        let class = if slot == ConfigSlot::Credentials {
            crate::ErrorClass::NoCredentials
        } else {
            crate::ErrorClass::InvalidConfig
        };
        Box::new(crate::Error::new(class).with_detail(slot))
    };
    let config_file_error = |diagnostic: String| {
        Box::new(
            crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(
                super::credentials::FileDiagnostic::from_text(
                    crate::FileSlot::Config,
                    selection.config_source,
                    &diagnostic,
                ),
            ),
        )
    };
    let file_error = |diagnostic: String| {
        Box::new(
            crate::Error::new(crate::ErrorClass::CredentialsFile).with_detail(
                super::credentials::FileDiagnostic::from_text(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                    &diagnostic,
                ),
            ),
        )
    };
    let config = if selection.config_present {
        let path = selection
            .config
            .as_ref()
            .ok_or_else(|| error(ConfigSlot::Config))?;
        Some(open(path, Slot::Config).map_err(&config_file_error)?)
    } else {
        if selection.config_named {
            return Err(config_file_error("config: not found".to_owned()));
        }
        None
    };
    let credentials = if selection.credentials_present {
        let path = selection
            .credentials
            .as_ref()
            .ok_or_else(|| error(ConfigSlot::Credentials))?;
        Some(open_without_permissions(path, Slot::Credentials).map_err(&file_error)?)
    } else {
        if selection.credentials_named {
            return Err(error(ConfigSlot::Credentials));
        }
        None
    };
    if let (Some(config), Some(credentials)) = (&config, &credentials)
        && same(config, credentials).map_err(|_| error(ConfigSlot::Config))?
    {
        return Err(error(ConfigSlot::Config));
    }
    if let Some(credentials) = &credentials {
        check_credentials_permissions(credentials).map_err(&file_error)?;
    }
    Ok(Pair {
        config,
        credentials,
    })
}

#[cfg(unix)]
fn credentials_identity(path: &Path, config: Option<&File>) -> Result<bool, &'static str> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "not found"
        } else {
            "metadata failed"
        }
    })?;
    if !metadata.is_file() {
        return Err("not a regular file");
    }
    let Some(config) = config else {
        return Ok(false);
    };
    let config = config.metadata().map_err(|_| "metadata failed")?;
    Ok((metadata.dev(), metadata.ino()) == (config.dev(), config.ino()))
}

#[cfg(windows)]
fn credentials_identity(path: &Path, config: Option<&File>) -> Result<bool, &'static str> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT);
    let file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "not found"
        } else {
            "metadata failed"
        }
    })?;
    if !winapi_util::file::typ(&file)
        .map_err(|_| "metadata failed")?
        .is_disk()
        || !file.metadata().map_err(|_| "metadata failed")?.is_file()
    {
        return Err("not a regular file");
    }
    config
        .map(|config| same(config, &file).map_err(|_| "metadata failed"))
        .unwrap_or(Ok(false))
}

#[cfg(not(any(unix, windows)))]
fn credentials_identity(_path: &Path, _config: Option<&File>) -> Result<bool, &'static str> {
    Err("metadata failed")
}

pub(super) fn open_settings(
    selection: &super::mode::Selection,
) -> Result<Option<File>, Box<crate::Error>> {
    let file_error = |slot, source, reason| {
        let class = if slot == crate::FileSlot::Config {
            crate::ErrorClass::InvalidConfig
        } else {
            crate::ErrorClass::CredentialsFile
        };
        Box::new(
            crate::Error::new(class).with_detail(crate::FileDiagnostic::from_text(
                slot,
                source,
                &format!("file: {reason}"),
            )),
        )
    };
    let config = if selection.config_present {
        let path = selection.config.as_ref().ok_or_else(|| {
            file_error(
                crate::FileSlot::Config,
                selection.config_source,
                "not found",
            )
        })?;
        Some(open(path, Slot::Config).map_err(|diagnostic| {
            Box::new(
                crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(
                    crate::FileDiagnostic::from_text(
                        crate::FileSlot::Config,
                        selection.config_source,
                        &diagnostic,
                    ),
                ),
            )
        })?)
    } else if selection.config_named {
        return Err(file_error(
            crate::FileSlot::Config,
            selection.config_source,
            "not found",
        ));
    } else {
        None
    };
    if selection.credentials_present {
        let path = selection.credentials.as_ref().ok_or_else(|| {
            file_error(
                crate::FileSlot::Credentials,
                selection.credentials_source,
                "not found",
            )
        })?;
        let alias = credentials_identity(path, config.as_ref()).map_err(|reason| {
            file_error(
                crate::FileSlot::Credentials,
                selection.credentials_source,
                reason,
            )
        })?;
        if alias {
            return Err(Box::new(crate::Error::new(
                crate::ErrorClass::InvalidConfig,
            )));
        }
    } else if selection.credentials_named {
        return Err(file_error(
            crate::FileSlot::Credentials,
            selection.credentials_source,
            "not found",
        ));
    }
    Ok(config)
}

#[cfg(test)]
mod tests;
