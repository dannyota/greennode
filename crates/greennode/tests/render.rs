use greennode::cli::render;

use serde::Serialize;
use serde::ser::{SerializeSeq, SerializeStruct};
use std::path::Path;

fn write_json<W: std::io::Write, T: ?Sized + Serialize>(
    w: &mut W,
    value: &T,
) -> Result<(), render::Error> {
    render::write_json(w, &render::project(value)?)
}

fn normalize<T: ?Sized + Serialize>(value: &T) -> Result<render::Value, render::Error> {
    render::project(value)?.for_query()
}

struct Ordered(&'static serde_json::Value);

impl Serialize for Ordered {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            serde_json::Value::Null => serializer.serialize_unit(),
            serde_json::Value::Bool(v) => serializer.serialize_bool(*v),
            serde_json::Value::Number(v) => v.serialize(serializer),
            serde_json::Value::String(v) => serializer.serialize_str(v),
            serde_json::Value::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&Ordered(value))?;
                }
                seq.end()
            }
            serde_json::Value::Object(values) => {
                let mut obj = serializer.serialize_struct("Golden", values.len())?;
                for (key, value) in values {
                    obj.serialize_field(key, &Ordered(value))?;
                }
                obj.end()
            }
        }
    }
}

#[test]
fn json_goldens_round_trip() {
    static GOLDENS: std::sync::OnceLock<Vec<(String, Vec<u8>, serde_json::Value)>> =
        std::sync::OnceLock::new();
    let goldens = GOLDENS.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                let name = path.file_name()?.to_str()?.to_owned();
                if !name.ends_with(".json.golden") {
                    return None;
                }
                let bytes = std::fs::read(path).unwrap();
                let value = serde_json::from_slice(&bytes).unwrap();
                Some((name, bytes, value))
            })
            .collect()
    });
    for (name, bytes, value) in goldens {
        let mut actual = Vec::new();
        write_json(&mut actual, &Ordered(value)).unwrap();
        if actual != *bytes {
            let offset = actual
                .iter()
                .zip(bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(bytes.len()));
            panic!("{name}: byte {offset}");
        }
    }
    assert_eq!(goldens.len(), 146);
}

fn nested_json(depth: usize) -> serde_json::Value {
    let mut v = serde_json::Value::Null;
    for _ in 0..depth {
        v = serde_json::Value::Array(vec![v]);
    }
    v
}

fn nested_tree(depth: usize) -> render::Value {
    let mut v = render::Value::Null;
    for _ in 0..depth {
        v = render::Value::Array(vec![v]);
    }
    v
}

fn check_depth_result<T>(result: Result<T, render::Error>, depth: usize) {
    if depth == 128 {
        assert!(result.is_ok());
    } else {
        assert!(matches!(result, Err(render::Error::DepthLimit)));
    }
}

#[test]
fn depth_limits_on_default_stack() {
    std::thread::spawn(|| {
        for depth in [128, 129] {
            let value = nested_json(depth);
            let mut out = Vec::new();
            let result = write_json(&mut out, &value);
            check_depth_result(result, depth);
            let result = normalize(&value);
            check_depth_result(result, depth);
            let tree = nested_tree(depth);
            let mut out = Vec::new();
            let result = render::write_text(&mut out, &tree);
            check_depth_result(result, depth);
            let mut out = Vec::new();
            let result = render::write_table(&mut out, &tree);
            check_depth_result(result, depth);
        }
    })
    .join()
    .unwrap();
}

#[test]
fn tuple_variant_depth_counts_both_containers() {
    struct Nested(usize);
    impl Serialize for Nested {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeTupleVariant;
            if self.0 == 0 {
                return serializer.serialize_unit();
            }
            let mut variant = serializer.serialize_tuple_variant("Nested", 0, "Next", 1)?;
            variant.serialize_field(&Nested(self.0 - 1))?;
            variant.end()
        }
    }
    let mut out = Vec::new();
    assert!(write_json(&mut out, &Nested(64)).is_ok());
    let tree = normalize(&Nested(64)).unwrap();
    assert!(render::write_text(&mut Vec::new(), &tree).is_ok());
    assert!(render::write_table(&mut Vec::new(), &tree).is_ok());
    assert!(matches!(
        write_json(&mut Vec::new(), &Nested(65)),
        Err(render::Error::DepthLimit)
    ));
    assert!(matches!(
        normalize(&Nested(65)),
        Err(render::Error::DepthLimit)
    ));
}

#[test]
fn rejects_map_key_without_value() {
    struct Bad;
    impl Serialize for Bad {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(None)?;
            map.serialize_key("first")?;
            map.serialize_key("second")?;
            map.serialize_value(&1)?;
            map.end()
        }
    }
    let mut out = Vec::new();
    assert!(write_json(&mut out, &Bad).is_err());
    assert!(out.is_empty());
}

#[test]
fn rejects_invalid_json_number_adapter() {
    struct Bad;
    impl Serialize for Bad {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut state = s.serialize_struct("$serde_json::private::Number", 1)?;
            state.serialize_field("$serde_json::private::Number", "oops")?;
            state.end()
        }
    }
    let mut out = Vec::new();
    assert!(write_json(&mut out, &Bad).is_err());
    assert!(out.is_empty());
}

#[test]
fn nil_and_empty_slices_and_maps() {
    #[derive(serde::Serialize)]
    struct Projection {
        nil_slice: Vec<i32>,
        nil_slice_pointer: Option<Vec<i32>>,
        empty_slice_pointer: Option<Vec<i32>>,
        nil_map: Option<std::collections::BTreeMap<String, i32>>,
        empty_map: std::collections::BTreeMap<String, i32>,
    }
    let projection = Projection {
        nil_slice: Vec::new(),
        nil_slice_pointer: None,
        empty_slice_pointer: Some(Vec::new()),
        nil_map: None,
        empty_map: std::collections::BTreeMap::new(),
    };
    let mut out = Vec::new();
    write_json(&mut out, &projection).unwrap();
    assert_eq!(out, b"{\n  \"nil_slice\": [],\n  \"nil_slice_pointer\": null,\n  \"empty_slice_pointer\": [],\n  \"nil_map\": null,\n  \"empty_map\": {}\n}\n");
}

#[test]
fn embedded_fields_flatten_in_declaration_order() {
    struct Embedded {
        z: i32,
        a: i32,
    }
    struct Record {
        before: i32,
        embedded: Option<Embedded>,
        after: i32,
    }
    impl Serialize for Record {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut object = serializer
                .serialize_struct("Record", if self.embedded.is_some() { 4 } else { 2 })?;
            object.serialize_field("before", &self.before)?;
            if let Some(inner) = &self.embedded {
                object.serialize_field("z", &inner.z)?;
                object.serialize_field("a", &inner.a)?;
            }
            object.serialize_field("after", &self.after)?;
            object.end()
        }
    }
    for (embedded, expected) in [
        (
            Some(Embedded { z: 2, a: 3 }),
            "{\n  \"before\": 1,\n  \"z\": 2,\n  \"a\": 3,\n  \"after\": 4\n}\n",
        ),
        (None, "{\n  \"before\": 1,\n  \"after\": 4\n}\n"),
    ] {
        let mut out = Vec::new();
        write_json(
            &mut out,
            &Record {
                before: 1,
                embedded,
                after: 4,
            },
        )
        .unwrap();
        assert_eq!(out, expected.as_bytes());
    }
}

#[test]
fn exact_integer_and_nonfinite_float() {
    for (number, expected) in [
        (9007199254740991_u64, "9007199254740991\n"),
        (u64::MAX, "18446744073709551615\n"),
    ] {
        let mut out = Vec::new();
        write_json(&mut out, &number).unwrap();
        assert_eq!(out, expected.as_bytes());
    }
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut out = Vec::new();
        assert!(write_json(&mut out, &number).is_err());
        assert!(out.is_empty());
    }
}

struct FloatKey<T>(T);

impl<T: Serialize> Serialize for FloatKey<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(&self.0, &0)?;
        map.end()
    }
}

fn oracle_equal(case: &str, actual: &[u8], expected: &[u8]) {
    if actual != expected {
        let offset = actual
            .iter()
            .zip(expected)
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        panic!("{case}: byte {offset}");
    }
}

fn compare_float<T: Serialize>(case: &str, value: T, json: &str, fixed: &str, key: &str) {
    let mut out = Vec::new();
    write_json(&mut out, &value).unwrap();
    oracle_equal(case, &out, format!("{json}\n").as_bytes());
    let tree = normalize(&value).unwrap();
    out.clear();
    render::write_text(&mut out, &tree).unwrap();
    oracle_equal(case, &out, format!("{fixed}\n").as_bytes());
    let width = fixed.chars().count().max(5);
    let border = format!("+{}+\n", "-".repeat(width + 2));
    let expected = format!(
        "{border}| Value{} |\n{border}| {fixed}{} |\n{border}",
        " ".repeat(width - 5),
        " ".repeat(width - fixed.len())
    );
    out.clear();
    render::write_table(&mut out, &tree).unwrap();
    oracle_equal(case, &out, expected.as_bytes());
    out.clear();
    write_json(&mut out, &FloatKey(value)).unwrap();
    oracle_equal(case, &out, format!("{{\n  \"{key}\": 0\n}}\n").as_bytes());
}

#[test]
fn floats_match_go_oracle() {
    let corpus = include_str!("fixtures/render/floats.tsv");
    let mut count = 0;
    for line in corpus.lines() {
        let mut fields = line.split('\t');
        let id = fields.next().unwrap();
        let json = fields.next().unwrap();
        let fixed = fields.next().unwrap();
        let key = fields.next().unwrap();
        assert!(fields.next().is_none());
        if let Some(bits) = id.strip_prefix("64:") {
            compare_float(
                id,
                f64::from_bits(u64::from_str_radix(bits, 16).unwrap()),
                json,
                fixed,
                key,
            );
        } else if let Some(bits) = id.strip_prefix("32:") {
            compare_float(
                id,
                f32::from_bits(u32::from_str_radix(bits, 16).unwrap()),
                json,
                fixed,
                key,
            );
        } else {
            panic!("invalid oracle case");
        }
        count += 1;
    }
    assert_eq!(count, 27611);
}

#[test]
fn float_ties_and_map_key_regressions() {
    let mut out = Vec::new();
    write_json(&mut out, &f64::from_bits(0x42d8_b5e7_eb11_7e68)).unwrap();
    oracle_equal("float-tie", &out, b"108678236358137.62\n");
    out.clear();
    write_json(&mut out, &FloatKey(1e-5_f64)).unwrap();
    oracle_equal("float-map-key", &out, b"{\n  \"1e-05\": 0\n}\n");
}

#[test]
fn float_notation_boundaries() {
    for (n, expected) in [
        (1e-7, "1e-7\n"),
        (1e-6, "0.000001\n"),
        (1e21, "1e+21\n"),
        (9007199254740992.0, "9007199254740992\n"),
        (-0.0, "-0\n"),
    ] {
        let mut bytes = Vec::new();
        write_json(&mut bytes, &n).unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
}

fn custom_case(case: &str) -> bool {
    matches!(
        case,
        "billing-list-payments"
            | "billing-list-transactions"
            | "numeric-filter"
            | "projection"
            | "cdn-get-web-accelerator"
            | "compute-list-flavor-prices"
            | "volume-list-volume-prices"
            | "storage-list-storage-prices"
            | "network-list-network-prices"
            | "discovery-partial"
            | "pricing-estimate"
            | "pricing-estimate-partial"
            | "pricing-estimate-unknown"
            | "monitor-quota-status"
            | "monitor-list-synthetic-packages"
            | "monitor-get-synthetic-quota"
            | "monitor-list-notification-packages"
            | "monitor-create-notification-quota"
            | "monitor-create-notification-quota-sms"
            | "monitor-get-notification-quota"
            | "monitor-get-notification-quota-sms"
            | "monitor-delete-notification-quota-email"
            | "monitor-delete-notification-quota-sms"
            | "monitor-set-notification-quota-auto-renew-email"
            | "monitor-set-notification-quota-auto-renew-sms"
    )
}

#[test]
fn text_goldens() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(case) = name.strip_suffix(".json.golden") else {
            continue;
        };
        if custom_case(case) {
            continue;
        }
        let text_path = dir.join(format!("{case}.text.golden"));
        if !text_path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let tree = normalize(&value).unwrap();
        let expected = std::fs::read(text_path).unwrap();
        let mut actual = Vec::new();
        render::write_text(&mut actual, &tree).unwrap();
        if actual != expected {
            let offset = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            failures.push(format!("{case}: byte {offset}"));
        }
        count += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join(", "));
    assert!(count > 0);
}

#[test]
fn table_goldens() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(case) = name.strip_suffix(".json.golden") else {
            continue;
        };
        if custom_case(case) {
            continue;
        }
        let table_path = dir.join(format!("{case}.table.golden"));
        if !table_path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let tree = normalize(&value).unwrap();
        let expected = std::fs::read(table_path).unwrap();
        let mut actual = Vec::new();
        render::write_table(&mut actual, &tree).unwrap();
        if actual != expected {
            let offset = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            failures.push(format!("{case}: byte {offset}"));
        }
        count += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join(", "));
    assert!(count > 0);
}

#[test]
fn projection_redacts_only_marked_dynamic_maps() {
    #[derive(Serialize)]
    struct Record {
        dynamic: render::DynamicMap<std::collections::BTreeMap<&'static str, &'static str>>,
        typed: std::collections::BTreeMap<&'static str, &'static str>,
    }
    let input = Record {
        dynamic: render::DynamicMap(std::collections::BTreeMap::from([(
            "token",
            "dynamic-secret",
        )])),
        typed: std::collections::BTreeMap::from([("token", "typed-value")]),
    };
    let projected = render::project(&input).unwrap();
    let mut out = Vec::new();
    render::write_json(&mut out, &projected).unwrap();
    assert!(!out.windows(14).any(|w| w == b"dynamic-secret"));
    assert!(out.windows(11).any(|w| w == b"typed-value"));
    let tree = projected.for_query().unwrap();
    let render::Value::Object(fields) = &tree else {
        panic!("expected object");
    };
    assert!(
        matches!(fields.get("dynamic"), Some(render::Value::Object(map)) if matches!(map.get("token"), Some(render::Value::String(s)) if s == "<redacted>"))
    );
    assert!(
        matches!(fields.get("typed"), Some(render::Value::Object(map)) if matches!(map.get("token"), Some(render::Value::String(s)) if s == "typed-value"))
    );
    out.clear();
    render::write_text(&mut out, &tree).unwrap();
    assert!(!out.windows(14).any(|w| w == b"dynamic-secret"));
    assert!(out.windows(11).any(|w| w == b"typed-value"));
    assert!(out.windows(8).any(|w| w == b"redacted"));
    out.clear();
    render::write_table(&mut out, &tree).unwrap();
    assert!(!out.windows(14).any(|w| w == b"dynamic-secret"));
    assert!(out.windows(11).any(|w| w == b"typed-value"));
}

#[test]
fn projection_redacts_nested_dynamic_objects_and_arrays() {
    let input = render::DynamicMap(serde_json::json!({
        "nested": {"secret": "canary-one", "name": "safe"},
        "items": [{"token": "canary-two", "name": "safe"}]
    }));
    let projected = render::project(&input).unwrap();
    let mut out = Vec::new();
    render::write_json(&mut out, &projected).unwrap();
    assert!(!out.windows(6).any(|w| w == b"canary"));
    assert!(out.windows(8).filter(|w| *w == b"redacted").count() == 2);
    let query = projected.for_query().unwrap();
    out.clear();
    render::write_text(&mut out, &query).unwrap();
    assert!(!out.windows(6).any(|w| w == b"canary"));
    assert!(out.windows(8).filter(|w| *w == b"redacted").count() == 2);
}

#[test]
fn redaction_unicode_keys() {
    let value = normalize(&render::DynamicMap(serde_json::json!({"pass\u{345}word": "x", "toKen": "x", "pass密word": "x", "author": "keep"}))).unwrap();
    let render::Value::Object(map) = value else {
        panic!("expected object");
    };
    for key in ["pass\u{345}word", "toKen", "pass密word"] {
        assert_eq!(
            map.get(key),
            Some(&render::Value::String("<redacted>".into()))
        );
    }
    assert_eq!(
        map.get("author"),
        Some(&render::Value::String("keep".into()))
    );
}

#[test]
fn redacts_dynamic_maps_at_depth() {
    let value = normalize(&render::DynamicMap(
        serde_json::json!({"safe": [{"API-Key": "secret", "author": "name"}]}),
    ))
    .unwrap();
    let mut out = Vec::new();
    render::write_text(&mut out, &value).unwrap();
    assert!(!out.windows(6).any(|s| s == b"secret"));
    assert!(out.windows(8).any(|s| s == b"redacted"));
    assert!(out.windows(4).any(|s| s == b"name"));
}

#[test]
fn table_rejects_column_limit_before_writing() {
    let map: std::collections::BTreeMap<_, _> = (0..101)
        .map(|i| (format!("k{i}"), render::Value::Null))
        .collect();
    let mut out = Vec::new();
    assert!(matches!(
        render::write_table(&mut out, &render::Value::Object(map)),
        Err(render::Error::TableLimit)
    ));
    assert!(out.is_empty());
    assert!(render::write_table(&mut out, &render::Value::Null).is_ok());
}

#[test]
fn table_rejects_cell_limit_before_writing() {
    let map: std::collections::BTreeMap<_, _> = (0..100)
        .map(|i| (format!("k{i}"), render::Value::Null))
        .collect();
    let rows = render::Value::Array(vec![render::Value::Object(map); 10001]);
    let mut out = Vec::new();
    assert!(matches!(
        render::write_table(&mut out, &rows),
        Err(render::Error::TableLimit)
    ));
    assert!(out.is_empty());
}

#[test]
fn table_rejects_projected_bytes_before_writing() {
    let value = render::Value::Array(vec![render::Value::String("a".repeat(1 << 20)); 64]);
    let mut out = Vec::new();
    assert!(matches!(
        render::write_table(&mut out, &value),
        Err(render::Error::TableLimit)
    ));
    assert!(out.is_empty());
}

#[test]
fn table_renders_heterogeneous_rows() {
    let value = serde_json::json!([{"a": "first"}, {"b": "second"}]);
    let tree = normalize(&value).unwrap();
    let mut bytes = Vec::new();
    render::write_table(&mut bytes, &tree).unwrap();
    assert_eq!(bytes, b"+-------+--------+\n| a     | b      |\n+-------+--------+\n| first |        |\n|       | second |\n+-------+--------+\n");
}

#[test]
fn text_renders_sorted_cells_and_items() {
    let value =
        serde_json::json!({"Items": [{"z": 7, "a": "hello\nthere"}, {"z": false, "a": null}]});
    let tree = normalize(&value).unwrap();
    let mut bytes = Vec::new();
    render::write_text(&mut bytes, &tree).unwrap();
    assert_eq!(bytes, b"hello\\nthere\t7\n\tfalse\n");
}

#[test]
fn escapes_json_keys_and_values() {
    let value = std::collections::BTreeMap::from([("a\u{009b}\u{202e}", "x\n<&\u{7f}\u{2028}")]);
    let mut out = Vec::new();
    write_json(&mut out, &value).unwrap();
    let expected = b"{\n  \"a\\u009b\\u202e\": \"x\\n\\u003c\\u0026\\u007f\\u2028\"\n}\n";
    if out != expected {
        let offset = out
            .iter()
            .zip(expected)
            .position(|(a, b)| a != b)
            .unwrap_or(out.len().min(expected.len()));
        panic!("escapes_json_keys_and_values: byte {offset}");
    }
}

#[test]
fn escapes_table_headers_and_cells() {
    let value = normalize(&serde_json::json!({"a\n\u{009b}": "x\t\u{001b}\u{202e}"})).unwrap();
    let mut out = Vec::new();
    render::write_table(&mut out, &value).unwrap();
    let rendered = String::from_utf8(out).unwrap();
    assert!(rendered.contains(r"a\n\x9b"));
    assert!(rendered.contains("x\\t\\x1b\\u202e"));
    assert!(!rendered.contains('\u{001b}'));
}

#[test]
fn sorted_map_and_nonfinite_rejection() {
    let value = std::collections::HashMap::from([("z", 1), ("a", 2)]);
    let mut out = Vec::new();
    write_json(&mut out, &value).unwrap();
    assert_eq!(out, b"{\n  \"a\": 2,\n  \"z\": 1\n}\n");
    out.clear();
    assert!(write_json(&mut out, &f64::INFINITY).is_err());
    assert!(out.is_empty());
}

#[test]
fn json_preserves_struct_order_and_integer_precision() {
    #[derive(serde::Serialize)]
    struct Record {
        z: u64,
        a: &'static str,
    }
    let mut output = Vec::new();
    write_json(
        &mut output,
        &Record {
            z: u64::MAX,
            a: "ok",
        },
    )
    .unwrap();
    assert_eq!(
        output,
        b"{\n  \"z\": 18446744073709551615,\n  \"a\": \"ok\"\n}\n"
    );
}
