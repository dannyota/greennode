#[cfg(unix)]
mod entry;
#[cfg(unix)]
mod lock;
#[cfg(unix)]
mod marker;
pub(crate) mod store;

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

#[allow(dead_code)]
pub(crate) fn iam_stem(
    profile: &str,
    root_email: &str,
    username: &str,
    signin_url: &str,
    token_url: &str,
) -> String {
    key_stem(&[profile, root_email, username, signin_url, token_url])
}

#[allow(dead_code)]
pub(crate) fn service_account_stem(
    profile: &str,
    client_id: &str,
    token_url: &str,
    secret: &crate::Secret,
) -> String {
    key_stem(&[
        "service-account",
        profile,
        client_id,
        token_url,
        secret.expose_secret(),
    ])
}

fn key_stem(fields: &[&str]) -> String {
    let mut digest = Sha256::new();
    for field in fields {
        digest.update(field.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(field.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[allow(dead_code)]
pub(crate) fn default_directory(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|home| home.join(".greennode").join("cache"))
}

#[allow(dead_code)]
pub(crate) fn checked_directory(
    home: Option<&Path>,
    path: &Path,
) -> Result<PathBuf, Box<crate::Error>> {
    let Some(home) = home else {
        return Ok(path.to_path_buf());
    };
    let unavailable = || Box::new(crate::Error::new(crate::ErrorClass::TokenCache));
    let legacy = resolve(&home.join(".vngcloud")).map_err(|()| unavailable())?;
    let directory = resolve(path).map_err(|()| unavailable())?;
    let mut components = directory.components();
    if legacy.components().all(|legacy_component| {
        components.next().is_some_and(|component| {
            component
                .as_os_str()
                .eq_ignore_ascii_case(legacy_component.as_os_str())
        })
    }) {
        return Err(unavailable());
    }
    Ok(directory)
}

fn resolve(path: &Path) -> Result<PathBuf, ()> {
    if !path.is_absolute() {
        return Err(());
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(());
                }
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = std::fs::canonicalize(&resolved).map_err(|_| ())?;
                        if !std::fs::metadata(&resolved).map_err(|_| ())?.is_dir() {
                            return Err(());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(()),
                }
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests;
