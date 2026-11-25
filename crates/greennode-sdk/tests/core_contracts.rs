use greennode_sdk::{ConfigSource, List, PagedList, Secret, Token};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

#[test]
fn secret_debug_uses_baseline_redaction() {
    let secret = Secret::new("toy-canary");
    let output = format!("{secret:?}");
    assert!(!output.contains(secret.expose_secret()));
    assert!(output == "[redacted]");
}

#[test]
fn token_refreshes_inside_the_freshness_window() {
    let now = OffsetDateTime::UNIX_EPOCH;
    let fresh = Token::new(Secret::new("toy"), Some(now + Duration::seconds(31)));
    let stale = Token::new(Secret::new("toy"), Some(now + Duration::seconds(29)));
    let empty = Token::new(Secret::new(""), Some(now + Duration::seconds(60)));
    assert!(empty.needs_refresh(now));
    assert!(!fresh.needs_refresh(now));
    assert!(stale.needs_refresh(now));
}

#[test]
fn secret_serialization_is_redacted_without_canary() {
    for value in ["toy-canary-serialize", ""] {
        let secret = Secret::new(value);
        let serialized = serde_json::to_string(&secret).unwrap();
        let debug = format!("{secret:?}");
        assert!(serialized == "\"[redacted]\"");
        assert!(debug == "[redacted]");
        if !value.is_empty() {
            assert!(!serialized.contains(value));
            assert!(!debug.contains(value));
        }
    }
}

#[test]
fn paging_defaults_match_baseline() {
    let page: i64 = greennode_sdk::DEFAULT_PAGE;
    let size: i64 = greennode_sdk::DEFAULT_PAGE_SIZE;
    assert_eq!((page, size), (1, 10_000));
}

#[test]
fn list_projection_preserves_names_order_and_empty_items() {
    let empty = List::<i32> { items: vec![] };
    let filled = List { items: vec![1, 2] };
    assert_eq!(serde_json::to_string(&empty).unwrap(), r#"{"Items":[]}"#);
    assert_eq!(
        serde_json::to_string(&filled).unwrap(),
        r#"{"Items":[1,2]}"#
    );
}

#[test]
fn paged_list_projection_preserves_names_order_and_empty_items() {
    let mut list = PagedList {
        items: vec![],
        page: -1,
        page_size: 2,
        total_page: 3,
        total_item: 4,
    };
    assert_eq!(
        serde_json::to_string(&list).unwrap(),
        r#"{"Items":[],"Page":-1,"PageSize":2,"TotalPage":3,"TotalItem":4}"#
    );
    list.items.push(5);
    assert_eq!(
        serde_json::to_string(&list).unwrap(),
        r#"{"Items":[5],"Page":-1,"PageSize":2,"TotalPage":3,"TotalItem":4}"#
    );
}

#[derive(Default)]
struct MapSource {
    values: HashMap<OsString, OsString>,
}
impl ConfigSource for MapSource {
    fn var_os(&self, name: &OsStr) -> Option<OsString> {
        self.values.get(name).cloned()
    }
}

#[test]
fn config_source_supplies_os_values_without_process_state() {
    let mut source = MapSource::default();
    source.values.insert("key".into(), "value".into());
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    assert_eq!(source.home_dir(), None);
    source.values.insert(key.into(), "".into());
    assert_eq!(source.home_dir(), None);
    source.values.insert(key.into(), "home".into());
    assert_eq!(source.var_os(OsStr::new("key")), Some("value".into()));
    assert_eq!(source.var_os(OsStr::new("missing")), None);
    assert_eq!(source.home_dir().as_deref(), Some(Path::new("home")));
}

#[test]
fn only_process_source_reads_process_environment() {
    fn visit(dir: &Path, excluded: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, excluded);
            } else if path.extension() == Some(OsStr::new("rs")) && path != excluded {
                assert!(
                    !std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("std::env::"),
                    "process access outside source.rs: {}",
                    path.display()
                );
            }
        }
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    visit(&src, &src.join("source.rs"));
}
