use super::*;
use std::{env, path::PathBuf};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "greennode-entry-{}-{:?}",
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

fn oversize_store() -> (Dir, Store) {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let path = dir.0.join("large.json");
    std::fs::write(&path, vec![b'x'; 65_537]).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(&dir.0).unwrap();
    (dir, store)
}

#[test]
fn oversized_iam_entry_is_error() {
    let (_dir, store) = oversize_store();
    assert!(
        matches!(read(&store, "large.json", Kind::Iam, OffsetDateTime::UNIX_EPOCH, None), Err(error) if error.class() == crate::ErrorClass::TokenCache),
        "cache.go:101,115"
    );
}

#[test]
fn oversized_service_account_entry_is_miss() {
    let (_dir, store) = oversize_store();
    assert!(
        matches!(
            read(
                &store,
                "large.json",
                Kind::ServiceAccount,
                OffsetDateTime::UNIX_EPOCH,
                None
            ),
            Ok(None)
        ),
        "service_account.go:73, service_account_test.go:149-167"
    );
}

#[test]
fn oversized_service_account_entry_can_be_replaced() {
    let (_dir, store) = oversize_store();
    let now = OffsetDateTime::UNIX_EPOCH;
    assert!(matches!(
        read(&store, "large.json", Kind::ServiceAccount, now, None),
        Ok(None)
    ));
    let entry = Entry {
        token: crate::Secret::new("toy"),
        expires: now + time::Duration::seconds(120),
        obtained: now,
    };
    let (context, _) = crate::RequestContext::new();
    assert!(write(&store, "large.json", &entry, &context, &crate::SystemClock).is_ok());
    assert!(matches!(
        read(&store, "large.json", Kind::ServiceAccount, now, None),
        Ok(Some(_))
    ));
}

#[test]
fn invalid_utf8_entry_is_miss() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new();
    let path = dir.0.join("invalid.json");
    let mut data = br#"{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z","extra":""#.to_vec();
    data.push(0xff);
    data.extend_from_slice(br#""}"#);
    std::fs::write(&path, data).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(&dir.0).unwrap();
    assert!(matches!(
        read(
            &store,
            "invalid.json",
            Kind::Iam,
            OffsetDateTime::UNIX_EPOCH,
            None
        ),
        Ok(None)
    ));
    assert!(matches!(
        read(
            &store,
            "missing.json",
            Kind::Iam,
            OffsetDateTime::UNIX_EPOCH,
            None
        ),
        Ok(None)
    ));
}

fn rejected(value: &str) {
    assert!(decode(&crate::Secret::new(value), OffsetDateTime::UNIX_EPOCH, None).is_none());
    let valid = crate::Secret::new(r#"{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z"}"#);
    assert!(decode(&valid, OffsetDateTime::UNIX_EPOCH, None).is_some());
}

#[test]
fn invalid_json_is_miss() {
    rejected("{");
}
#[test]
fn wrong_type_is_miss() {
    rejected(r#"{"accessToken":17}"#);
}
#[test]
fn nested_duplicate_is_miss() {
    rejected(r#"{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z","extra":{"x":1,"x":2}}"#);
}
#[test]
fn duplicate_field_is_miss() {
    rejected(r#"{"accessToken":"one","accessToken":"two","expiresAt":"2100-01-01T00:00:00Z"}"#);
}
#[test]
fn duplicate_folded_field_is_miss() {
    rejected(r#"{"accessToken":"one","ACCESSTOKEN":"two","expiresAt":"2100-01-01T00:00:00Z"}"#);
}
#[test]
fn escaped_token_is_miss() {
    rejected(&format!(
        r#"{{"accessToken":"tok{}u0065n","expiresAt":"2100-01-01T00:00:00Z","obtainedAt":"2020-01-01T00:00:00Z"}}"#,
        char::from(92)
    ));
}
#[test]
fn invalid_expiry_is_miss() {
    rejected(r#"{"accessToken":"toy","expiresAt":"invalid"}"#);
}
#[test]
fn empty_token_is_miss() {
    rejected(r#"{"accessToken":"","expiresAt":"2100-01-01T00:00:00Z"}"#);
}
#[test]
fn expired_token_is_miss() {
    rejected(r#"{"accessToken":"toy","expiresAt":"1970-01-01T00:00:30Z"}"#);
}

#[test]
fn escaped_timestamp_is_miss() {
    let slash = char::from(92);
    rejected(&format!(
        r#"{{"accessToken":"toy","expiresAt":"{slash}u0032100-01-01T00:00:00Z"}}"#
    ));
    rejected(&format!(
        r#"{{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z","obtainedAt":"{slash}u0031970-01-01T00:00:00Z"}}"#
    ));
}

#[test]
fn unknown_fields_are_allowed() {
    let raw = crate::Secret::new(
        r#"{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z","unknown":{"array":[1,true,null,"text"]}}"#,
    );
    assert!(decode(&raw, OffsetDateTime::UNIX_EPOCH, None).is_some());
}

#[test]
fn cancellation_before_token_rename_does_not_publish() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    let now = OffsetDateTime::UNIX_EPOCH;
    let entry = Entry {
        token: crate::Secret::new("toy"),
        expires: now + time::Duration::seconds(120),
        obtained: now,
    };
    let (context, cancel) = crate::RequestContext::new();
    let checks = std::cell::Cell::new(0);
    let result = write_with_guard(&store, "entry.json", &entry, || {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            cancel.cancel();
        }
        context
            .check(&crate::SystemClock)
            .map_or(Ok(()), |end| Err(Box::new(end.into())))
    });
    assert_eq!(checks.get(), 3);
    assert!(matches!(result, Err(error) if error.class() == crate::ErrorClass::Cancelled));
    assert!(store.read("entry.json").unwrap().is_none());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    let (active, _) = crate::RequestContext::new();
    assert!(write(&store, "entry.json", &entry, &active, &crate::SystemClock).is_ok());
}

#[test]
fn round_trip_and_redaction() {
    let dir = Dir::new();
    let store = Store::open(&dir.0).unwrap();
    let now = OffsetDateTime::UNIX_EPOCH;
    let entry = Entry {
        token: crate::Secret::new("toy-canary"),
        expires: now + time::Duration::seconds(120),
        obtained: now,
    };
    let (context, _) = crate::RequestContext::new();
    write(&store, "entry.json", &entry, &context, &crate::SystemClock).unwrap();
    let data = store.read("entry.json").unwrap().unwrap();
    let shape: serde_json::Value = serde_json::from_str(data.expose_secret()).unwrap();
    assert!(shape.get("accessToken").is_some());
    assert!(shape.get("expiresAt").is_some());
    assert!(shape.get("obtainedAt").is_some());
    let recovered = decode(&data, now, None).unwrap();
    assert!(recovered.token.expose_secret() == "toy-canary");
    assert_eq!(recovered.expires, entry.expires);
    assert_eq!(recovered.obtained, entry.obtained);
    assert!(!format!("{recovered:?}").contains("toy-canary"));
    let unsafe_entry = Entry {
        token: crate::Secret::new("toy-canary\""),
        expires: entry.expires,
        obtained: entry.obtained,
    };
    let error = write(
        &store,
        "invalid.json",
        &unsafe_entry,
        &context,
        &crate::SystemClock,
    )
    .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("toy-canary"));
}

#[test]
fn rejected_token_uses_age_boundary() {
    let raw = crate::Secret::new(
        r#"{"accessToken":"toy","expiresAt":"2100-01-01T00:00:00Z","obtainedAt":"1970-01-01T00:00:00Z"}"#,
    );
    let token = crate::Secret::new("toy");
    let now = OffsetDateTime::UNIX_EPOCH;
    assert!(decode(&raw, now + time::Duration::seconds(29), Some(&token)).is_some());
    assert!(decode(&raw, now + time::Duration::seconds(30), Some(&token)).is_none());
    assert!(decode(&raw, now - time::Duration::seconds(1), Some(&token)).is_none());
}
