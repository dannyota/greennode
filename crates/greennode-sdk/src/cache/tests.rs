use super::*;
use std::env;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-cache-path-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn baseline_key_vectors() {
    assert_eq!(
        iam_stem(
            "default",
            "root@example.test",
            "user",
            "https://signin.test",
            "https://token.test"
        ),
        "4298de6949eac86259cdd0e2b0762d1140bbe7308386120d6b32e8e429e7f3bc"
    );
    assert_eq!(
        service_account_stem(
            "default",
            "client",
            "https://token.test",
            &crate::Secret::new("toy-secret")
        ),
        "9c7bf9262590cdec0cb67910c4cfce93d8edea7a34ae99399d7f757a4213fef9"
    );
    assert_ne!(key_stem(&["ab", "c"]), key_stem(&["a", "bc"]));
}

#[test]
fn selects_default_or_explicit_directory() {
    let dir = Dir::new();
    assert_eq!(
        default_directory(Some(&dir.0)),
        Some(dir.0.join(".greennode/cache"))
    );
    assert_eq!(default_directory(None), None);
    let explicit = Path::new("relative/cache");
    assert_eq!(checked_directory(None, explicit).unwrap(), explicit);
    assert!(checked_directory(Some(&dir.0), &dir.0.join(".vngcloud-other/cache")).is_ok());
}

#[test]
fn refuses_legacy_directory() {
    let dir = Dir::new();
    let legacy = dir.0.join(".vngcloud");
    std::fs::create_dir(&legacy).unwrap();
    assert!(checked_directory(Some(&dir.0), &legacy).is_err());
    assert!(checked_directory(Some(&dir.0), &dir.0.join("cache")).is_ok());
}

#[test]
fn refuses_legacy_directory_with_different_case() {
    let dir = Dir::new();
    assert!(checked_directory(Some(&dir.0), &dir.0.join(".VNGCLOUD/cache")).is_err());
    assert!(checked_directory(Some(&dir.0), &dir.0.join(".VNGCLOUD-other/cache")).is_ok());
}

#[test]
fn refuses_missing_legacy_descendant() {
    let dir = Dir::new();
    let cache = dir.0.join(".vngcloud").join("missing").join("cache");
    assert!(checked_directory(Some(&dir.0), &cache).is_err());
    assert!(checked_directory(Some(&dir.0), &dir.0.join("missing/cache")).is_ok());
}

#[cfg(unix)]
#[test]
fn checked_directory_preserves_final_symlink_for_open() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = Dir::new();
    let target = dir.0.join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = dir.0.join("cache");
    symlink(&target, &link).unwrap();
    let checked = checked_directory(Some(&dir.0), &link).unwrap();
    assert!(store::Store::open(&checked).is_err());
    let checked_target = checked_directory(Some(&dir.0), &target).unwrap();
    assert!(store::Store::open(&checked_target).is_ok());
}

#[cfg(unix)]
#[test]
fn checked_directory_refuses_shared_permissions_at_open() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let cache = dir.0.join("cache");
    std::fs::create_dir(&cache).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();
    let checked = checked_directory(Some(&dir.0), &cache).unwrap();
    assert!(store::Store::open(&checked).is_err());
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(store::Store::open(&checked).is_ok());
}

#[cfg(unix)]
#[test]
fn identity_check_detects_legacy_alias() {
    use std::os::unix::fs::symlink;
    let dir = Dir::new();
    let legacy = dir.0.join(".vngcloud");
    std::fs::create_dir(&legacy).unwrap();
    let alias = dir.0.join("alias");
    symlink(&legacy, &alias).unwrap();
    assert!(inside_legacy_identity(&legacy, &alias.join("missing")).unwrap());
    assert!(checked_directory(Some(&dir.0), &alias).is_err());
    assert!(!inside_legacy_identity(&legacy, &dir.0.join("other")).unwrap());
}

#[cfg(unix)]
#[test]
fn refuses_symlink_to_legacy_directory() {
    let dir = Dir::new();
    let legacy = dir.0.join(".vngcloud");
    std::fs::create_dir(&legacy).unwrap();
    let link = dir.0.join("alias");
    std::os::unix::fs::symlink(&legacy, &link).unwrap();
    assert!(checked_directory(Some(&dir.0), &link.join("cache")).is_err());
    assert!(checked_directory(Some(&dir.0), &dir.0.join("cache")).is_ok());
}
