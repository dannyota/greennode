use super::store::Store;

#[allow(dead_code)]
pub(crate) fn read(store: &Store, name: &str, current: i64) -> Option<i64> {
    match store.read_marker(name) {
        Ok(None) => None,
        Ok(Some(bytes)) => Some(
            std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|step| *step >= 0)
                .unwrap_or(current),
        ),
        Err(_) => Some(current),
    }
}

#[allow(dead_code)]
pub(crate) fn write_hook(
    store: &Store,
    name: &str,
    step: i64,
    context: &crate::RequestContext,
    clock: &dyn crate::Clock,
    live: &std::sync::atomic::AtomicBool,
) -> Result<(), Box<crate::Error>> {
    store.write_guarded_hook(
        name,
        &crate::Secret::new(step.to_string()),
        || {
            context
                .check(clock)
                .map_or(Ok(()), |end| Err(Box::new(end.into())))
        },
        None,
        Some(live),
    )
}

#[allow(dead_code)]
pub(crate) fn write(
    store: &Store,
    name: &str,
    step: i64,
    context: &crate::RequestContext,
    clock: &dyn crate::Clock,
) -> Result<(), Box<crate::Error>> {
    store.write_guarded(name, &crate::Secret::new(step.to_string()), || {
        context
            .check(clock)
            .map_or(Ok(()), |end| Err(Box::new(end.into())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, path::PathBuf};

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "greennode-marker-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir(&path).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn cancellation_before_rename_leaves_no_marker() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let checks = std::cell::Cell::new(0);
        let result = store.write_guarded("entry.totp", &crate::Secret::new("123"), || {
            checks.set(checks.get() + 1);
            if checks.get() == 2 {
                Err(Box::new(crate::Error::from(crate::ContextEnd::Cancelled)))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(read(&store, "entry.totp", 42), None);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
        let (context, _) = crate::RequestContext::new();
        write(&store, "entry.totp", 123, &context, &crate::SystemClock).unwrap();
        assert_eq!(read(&store, "entry.totp", 42), Some(123));
    }

    #[test]
    fn cancelled_marker_write_changes_nothing() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let (context, cancel) = crate::RequestContext::new();
        cancel.cancel();
        assert!(write(&store, "entry.totp", 123, &context, &crate::SystemClock).is_err());
        assert_eq!(read(&store, "entry.totp", 42), None);
    }

    #[test]
    fn marker_round_trip_and_prefix() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let (context, _) = crate::RequestContext::new();
        write(&store, "entry.totp", 123, &context, &crate::SystemClock).unwrap();
        assert_eq!(read(&store, "entry.totp", 42), Some(123));
        store
            .write(
                "entry.totp",
                &crate::Secret::new(format!("{}bad", "0".repeat(32))),
            )
            .unwrap();
        assert_eq!(read(&store, "entry.totp", 42), Some(0));
    }

    #[test]
    fn symlink_marker_is_replaced_without_touching_target() {
        use std::os::unix::fs::symlink;
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        let target = dir.0.join("target");
        std::fs::write(&target, "42").unwrap();
        symlink(&target, dir.0.join("entry.totp")).unwrap();
        let (context, _) = crate::RequestContext::new();
        assert!(write(&store, "entry.totp", 123, &context, &crate::SystemClock).is_ok());
        assert_eq!(read(&store, "entry.totp", 0), Some(123));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "42");
        assert!(
            !std::fs::symlink_metadata(dir.0.join("entry.totp"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn unsafe_marker_uses_current_step() {
        use std::os::unix::fs::symlink;
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        symlink("missing", dir.0.join("entry.totp")).unwrap();
        assert_eq!(read(&store, "entry.totp", 42), Some(42));
        assert_eq!(read(&store, "missing.totp", 42), None);
    }

    #[test]
    fn malformed_marker_uses_current_step() {
        let dir = Dir::new();
        let store = Store::open(&dir.0).unwrap();
        for value in ["invalid", "-1", "1\n", "9223372036854775808"] {
            store
                .write("entry.totp", &crate::Secret::new(value))
                .unwrap();
            assert_eq!(read(&store, "entry.totp", 42), Some(42));
        }
        assert_eq!(read(&store, "missing.totp", 42), None);
    }
}
