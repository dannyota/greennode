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
    open_without_permissions_checked(path, slot, &|| false)
}

fn open_without_permissions_checked(
    path: &Path,
    slot: Slot,
    check: &dyn Fn() -> bool,
) -> Result<File, String> {
    let error = |reason| format!("{}: {reason}", slot.label());
    if check() {
        return Err(error("open failed"));
    }
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
    if check() {
        return Err(error("metadata failed"));
    }
    #[cfg(windows)]
    if !winapi_util::file::typ(&file)
        .map_err(|_| error("metadata failed"))?
        .is_disk()
    {
        return Err(error("not a regular file"));
    }
    if check() {
        return Err(error("metadata failed"));
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
type Identity = (u64, u64);
#[cfg(windows)]
type Identity = same_file::Handle;
#[cfg(not(any(unix, windows)))]
type Identity = ();

#[cfg(unix)]
fn identity_file(file: &File, check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    if check() {
        return Err("metadata failed");
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| "metadata failed")?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
fn identity_path(path: &Path, check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    if check() {
        return Err("metadata failed");
    }
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
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity_file(file: &File, check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    if check() {
        return Err("metadata failed");
    }
    let file = file.try_clone().map_err(|_| "metadata failed")?;
    if check() {
        return Err("metadata failed");
    }
    same_file::Handle::from_file(file).map_err(|_| "metadata failed")
}

#[cfg(windows)]
fn identity_path(path: &Path, check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    if check() {
        return Err("metadata failed");
    }
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT);
    if check() {
        return Err("metadata failed");
    }
    let file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "not found"
        } else {
            "metadata failed"
        }
    })?;
    if check() {
        return Err("metadata failed");
    }
    if !winapi_util::file::typ(&file)
        .map_err(|_| "metadata failed")?
        .is_disk()
    {
        return Err("not a regular file");
    }
    if check() {
        return Err("metadata failed");
    }
    if !file.metadata().map_err(|_| "metadata failed")?.is_file() {
        return Err("not a regular file");
    }
    if check() {
        return Err("metadata failed");
    }
    same_file::Handle::from_file(file).map_err(|_| "metadata failed")
}

#[cfg(not(any(unix, windows)))]
fn identity_path(_path: &Path, _check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    Err("metadata failed")
}
#[cfg(not(any(unix, windows)))]
fn identity_file(_file: &File, _check: &dyn Fn() -> bool) -> Result<Identity, &'static str> {
    Err("metadata failed")
}

#[cfg(test)]
pub(super) fn open_settings(
    selection: &super::mode::Selection,
) -> Result<Option<File>, Box<crate::Error>> {
    open_settings_with(selection, || {})
}

#[cfg(test)]
fn open_settings_with(
    selection: &super::mode::Selection,
    before_open: impl FnOnce(),
) -> Result<Option<File>, Box<crate::Error>> {
    open_settings_guarded(selection, before_open, || None, || {})
}

pub(super) fn open_settings_checked(
    selection: &super::mode::Selection,
    check: impl Fn() -> Option<crate::ContextEnd>,
) -> Result<Option<File>, Box<crate::Error>> {
    open_settings_guarded(selection, || {}, check, || {})
}

fn open_settings_guarded(
    selection: &super::mode::Selection,
    before_open: impl FnOnce(),
    check: impl Fn() -> Option<crate::ContextEnd>,
    on_open: impl FnOnce(),
) -> Result<Option<File>, Box<crate::Error>> {
    let guard = || check().map_or(Ok(()), |end| Err(Box::new(crate::Error::from(end))));
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
    let invalid = || Box::new(crate::Error::new(crate::ErrorClass::InvalidConfig));
    let changed = |slot, source| {
        Box::new(
            crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(
                crate::FileDiagnostic::from_text(slot, source, "file: changed during read"),
            ),
        )
    };
    let selected_path = |path: &Option<std::path::PathBuf>,
                         present,
                         named,
                         slot,
                         source|
     -> Result<Option<std::path::PathBuf>, Box<crate::Error>> {
        if present {
            path.clone()
                .map(Some)
                .ok_or_else(|| file_error(slot, source, "not found"))
        } else if named {
            Err(file_error(slot, source, "not found"))
        } else {
            Ok(None)
        }
    };
    let config_path = selected_path(
        &selection.config,
        selection.config_present,
        selection.config_named,
        crate::FileSlot::Config,
        selection.config_source,
    )?;
    let credentials_path = selected_path(
        &selection.credentials,
        selection.credentials_present,
        selection.credentials_named,
        crate::FileSlot::Credentials,
        selection.credentials_source,
    )?;
    guard()?;
    let config_identity = config_path
        .as_deref()
        .map(|path| identity_path(path, &|| check().is_some()))
        .transpose();
    guard()?;
    let config_identity = config_identity
        .map_err(|reason| file_error(crate::FileSlot::Config, selection.config_source, reason))?;
    guard()?;
    let credentials_identity = credentials_path
        .as_deref()
        .map(|path| identity_path(path, &|| check().is_some()))
        .transpose();
    guard()?;
    let credentials_identity = credentials_identity.map_err(|reason| {
        file_error(
            crate::FileSlot::Credentials,
            selection.credentials_source,
            reason,
        )
    })?;
    if config_identity.is_some() && config_identity == credentials_identity {
        return Err(invalid());
    }
    before_open();
    guard()?;
    if credentials_identity.is_none()
        && let Some(path) = selection.credentials.as_deref()
    {
        let observed = identity_path(path, &|| check().is_some());
        guard()?;
        match observed {
            Err("not found") => {}
            Ok(_) => {
                return Err(changed(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                ));
            }
            Err(reason) => {
                return Err(file_error(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                    reason,
                ));
            }
        }
    }
    guard()?;
    if config_path.is_some() {
        on_open();
    }
    let config = config_path
        .as_deref()
        .map(|path| open_without_permissions_checked(path, Slot::Config, &|| check().is_some()))
        .transpose();
    guard()?;
    let config = config.map_err(|diagnostic| {
        Box::new(
            crate::Error::new(crate::ErrorClass::InvalidConfig).with_detail(
                crate::FileDiagnostic::from_text(
                    crate::FileSlot::Config,
                    selection.config_source,
                    &diagnostic,
                ),
            ),
        )
    })?;
    guard()?;
    let opened_identity = config
        .as_ref()
        .map(|file| identity_file(file, &|| check().is_some()))
        .transpose();
    guard()?;
    let opened_identity = opened_identity.map_err(|_| {
        file_error(
            crate::FileSlot::Config,
            selection.config_source,
            "metadata failed",
        )
    })?;
    if opened_identity != config_identity
        || (opened_identity.is_some() && opened_identity == credentials_identity)
    {
        return Err(changed(crate::FileSlot::Config, selection.config_source));
    }
    if let Some(path) = selection.credentials.as_deref() {
        guard()?;
        let after = identity_path(path, &|| check().is_some());
        guard()?;
        match after {
            Ok(after) if Some(&after) != credentials_identity.as_ref() => {
                return Err(changed(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                ));
            }
            Ok(_) => {}
            Err("not found") if credentials_identity.is_none() => {}
            Err("not found") => {
                return Err(changed(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                ));
            }
            Err(reason) => {
                return Err(file_error(
                    crate::FileSlot::Credentials,
                    selection.credentials_source,
                    reason,
                ));
            }
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests;
