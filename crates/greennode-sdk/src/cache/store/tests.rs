use super::*;
use std::env;
use std::path::PathBuf;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(std::fs::canonicalize(path).unwrap())
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn windows_store_is_unavailable() {
    let dir = Dir::new();
    assert!(Store::open(&dir.0).is_err());
}

#[cfg(unix)]
#[test]
fn creates_cache_and_namespace() {
    let dir = Dir::new();
    let cache = dir.0.join("cache");
    let store = Store::create(&cache).unwrap();
    let _namespace = store.create_child("service-account-v1").unwrap();
    assert!(cache.join("service-account-v1").is_dir());
}

#[cfg(unix)]
#[test]
fn rejects_ancestor_symlink_into_legacy_tree() {
    let dir = Dir::new();
    let legacy = dir.0.join(".vngcloud");
    std::fs::create_dir(&legacy).unwrap();
    std::os::unix::fs::symlink(&legacy, dir.0.join("redirect")).unwrap();
    assert!(Store::create(&dir.0.join("redirect/cache")).is_err());
    assert!(!legacy.join("cache").exists());
    assert!(Store::create(&dir.0.join("safe")).is_ok());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_namespace() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    std::fs::create_dir(dir.0.join("target")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.0.join("target"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink("target", dir.0.join("service-account-v1")).unwrap();
    assert!(store.create_child("service-account-v1").is_err());
    assert!(store.create_child("other").is_ok());
}

#[cfg(unix)]
#[test]
fn rejects_group_writable_namespace() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    std::fs::create_dir(dir.0.join("service-account-v1")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        dir.0.join("service-account-v1"),
        std::fs::Permissions::from_mode(0o770),
    )
    .unwrap();
    assert!(store.create_child("service-account-v1").is_err());
    std::fs::set_permissions(
        dir.0.join("service-account-v1"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(store.create_child("service-account-v1").is_ok());
}

#[cfg(unix)]
#[test]
fn missing_entry_is_a_cache_miss() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    assert!(store.read("missing.json").unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn round_trip() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    store
        .write("entry.json", &crate::Secret::new("toy-token"))
        .unwrap();
    assert!(store.read("entry.json").unwrap().unwrap().expose_secret() == "toy-token");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(dir.0.join("entry.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
    store
        .write("entry.json", &crate::Secret::new("replacement"))
        .unwrap();
    assert!(store.read("entry.json").unwrap().unwrap().expose_secret() == "replacement");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_file() {
    let dir = Dir::new();
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(dir.0.join("target"), "toy").unwrap();
    std::fs::set_permissions(dir.0.join("target"), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink("target", dir.0.join("entry.json")).unwrap();
    let store = Store::open(&dir.0).unwrap();
    assert!(store.read("entry.json").is_err());
}

#[cfg(unix)]
#[test]
fn replacement_directory_does_not_redirect_write() {
    let dir = Dir::new();
    let original = dir.0.join("cache");
    let moved = dir.0.join("moved");
    let target = dir.0.join("target");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&target).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(&original).unwrap();
    std::fs::rename(&original, &moved).unwrap();
    std::os::unix::fs::symlink(&target, &original).unwrap();
    store
        .write("entry.json", &crate::Secret::new("toy"))
        .unwrap();
    assert!(moved.join("entry.json").exists());
    assert!(!target.join("entry.json").exists());
}

#[cfg(unix)]
#[test]
fn rejects_group_writable_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o770)).unwrap();
    assert!(Store::open(&dir.0).is_err());
    std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(Store::open(&dir.0).is_ok());
}

#[cfg(unix)]
#[test]
fn rejects_group_readable_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let file = dir.0.join("entry.json");
    std::fs::write(&file, "toy").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let store = Store::open(&dir.0).unwrap();
    assert!(store.read("entry.json").is_err());
}

#[cfg(unix)]
#[test]
fn rejects_oversized_file() {
    let dir = Dir::new();
    let file = dir.0.join("entry.json");
    std::fs::write(&file, vec![b'x'; 65_537]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let store = Store::open(&dir.0).unwrap();
    assert!(store.read("entry.json").is_err());
}

#[cfg(unix)]
#[test]
fn debug_and_error_do_not_disclose_canary() {
    let dir = Dir::new();
    let path = dir.0.join("toy-canary-token");
    std::fs::create_dir(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(&path).unwrap();
    assert!(!format!("{store:?}").contains("toy-canary-token"));
    store
        .write("entry.json", &crate::Secret::new("toy-canary-token"))
        .unwrap();
    let token = store.read("entry.json").unwrap().unwrap();
    assert!(!format!("{token:?}").contains("toy-canary-token"));
    let error = store.read("../toy-canary-token").unwrap_err();
    assert!(!format!("{error:?} {error}").contains("toy-canary-token"));
}
