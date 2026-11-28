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

#[derive(Clone, Copy)]
enum Coverage {
    Compared,
    Excluded(&'static str),
    Absent,
}

struct Golden {
    case: &'static str,
    separator: char,
    json: Coverage,
    table: Coverage,
    text: Coverage,
}

use Coverage::{Absent as A, Compared as C, Excluded as E};

const fn golden(
    case: &'static str,
    separator: char,
    json: Coverage,
    table: Coverage,
    text: Coverage,
) -> Golden {
    Golden {
        case,
        separator,
        json,
        table,
        text,
    }
}

const FLAVOR_DISCOVERY: Coverage =
    E("internal/cli/discovery_test.go:148-151: custom discovery view");
const NETWORK_DISCOVERY: Coverage =
    E("internal/cli/discovery_test.go:203-206: custom discovery view");
const DISCOVERY: Coverage = E("internal/cli/discovery_test.go:62-83: custom discovery view");
const ESTIMATE: Coverage = E("internal/cli/estimate_test.go:103-107: custom estimate view");
const CDN_QUERY: Coverage =
    E("internal/cli/svc_cdn_vcdn_test.go:99-101: table/text query differs from JSON");
const QUOTA_COVERAGE: Coverage =
    E("internal/cli/svc_monitor_quota_coverage_test.go:139-146: custom quota view");
const QUOTA_READ: Coverage = E("internal/cli/svc_monitor_test.go:1719-1758: custom quota view");
const QUOTA_WRITE: Coverage = E("internal/cli/svc_monitor_test.go:2215-2236: custom quota view");
const QUOTA_RENEW: Coverage = E("internal/cli/svc_monitor_test.go:2687-2706: custom quota view");
const QUOTA_SMS: Coverage = E("internal/cli/svc_monitor_test.go:2903-2982: custom quota view");
const NO_PRODUCER: Coverage = E("no producer at v1.0.1");

const INVENTORY: &[Golden] = &[
    golden("any-fields", '.', C, C, C),
    golden("billing-list-payments", '.', C, NO_PRODUCER, NO_PRODUCER),
    golden(
        "billing-list-transactions",
        '.',
        C,
        NO_PRODUCER,
        NO_PRODUCER,
    ),
    golden("cdn-get-web-accelerator", '.', C, CDN_QUERY, CDN_QUERY),
    golden("cdn-list-ip-ranges", '.', C, C, C),
    golden("compute-create-server", '.', C, C, A),
    golden("compute-create-server-floating-ip", '.', C, C, C),
    golden(
        "compute-create-server-floating-ip-conservative",
        '.',
        C,
        C,
        C,
    ),
    golden("compute-create-server-floating-ip-query", '.', A, A, C),
    golden("compute-delete-server", '.', C, C, A),
    golden(
        "compute-list-flavor-prices",
        '.',
        C,
        FLAVOR_DISCOVERY,
        FLAVOR_DISCOVERY,
    ),
    golden("compute-list-flavor-zones", '.', C, C, A),
    golden("compute-list-flavors", '.', C, C, A),
    golden("compute-quote-create-server", '.', C, C, A),
    golden("compute-quote-create-server-floating-ip", '.', C, C, C),
    golden(
        "compute-quote-create-server-floating-ip-conservative",
        '.',
        C,
        C,
        C,
    ),
    golden("compute-reboot-server", '.', C, C, A),
    golden("compute-rename-server", '.', C, C, A),
    golden("compute-resize-credit", '.', C, C, C),
    golden("compute-resize-server", '.', C, C, A),
    golden("compute-resize-server-credit", '.', C, C, C),
    golden("compute-start-server", '.', C, C, A),
    golden("compute-stop-server", '.', C, C, A),
    golden("containerregistry-create-repository", '.', C, C, A),
    golden("containerregistry-create-user", '.', C, C, A),
    golden("containerregistry-delete-repository", '.', C, C, A),
    golden("containerregistry-delete-user", '.', C, C, A),
    golden("containerregistry-get-repository", '.', C, C, A),
    golden("containerregistry-list-permissions", '.', C, C, A),
    golden("containerregistry-list-repositories", '.', C, C, A),
    golden("containerregistry-list-repository-users", '.', C, C, A),
    golden("containerregistry-list-users", '.', C, C, A),
    golden("control-chars", '.', C, C, C),
    golden("discovery-partial", '.', C, DISCOVERY, DISCOVERY),
    golden("dns-create-hosted-zone", '.', C, C, C),
    golden("dns-create-record", '.', C, C, C),
    golden("dns-delete-hosted-zone", '.', C, C, C),
    golden("dns-delete-record", '.', C, C, C),
    golden("dns-update-hosted-zone", '.', C, C, C),
    golden("dns-update-record", '.', C, C, C),
    golden("embedded-struct", '.', C, C, C),
    golden("get", '.', C, C, C),
    golden("globalloadbalancer-get-load-balancer", '.', C, C, A),
    golden("globalloadbalancer-list-load-balancers", '.', C, C, A),
    golden("iam-create-user", '-', C, C, C),
    golden("iam-delete-user", '-', C, A, A),
    golden("iam-disable-user-mfa", '-', C, C, C),
    golden("iam-enable-user-mfa", '-', C, C, C),
    golden("iam-get-user", '-', C, C, C),
    golden("iam-reset-user-password-generated", '-', C, C, C),
    golden("iam-reset-user-password-supplied", '-', C, C, C),
    golden("loadbalancer-add-pool-member", '.', C, C, A),
    golden("loadbalancer-create-listener", '.', C, C, A),
    golden("loadbalancer-create-load-balancer", '.', C, C, A),
    golden("loadbalancer-create-policy", '.', C, C, A),
    golden("loadbalancer-create-pool", '.', C, C, A),
    golden("loadbalancer-delete-listener", '.', C, C, A),
    golden("loadbalancer-delete-load-balancer", '.', C, C, A),
    golden("loadbalancer-delete-policy", '.', C, C, A),
    golden("loadbalancer-delete-pool", '.', C, C, A),
    golden("loadbalancer-get-load-balancer", '.', C, C, A),
    golden("loadbalancer-list-load-balancers", '.', C, C, A),
    golden("loadbalancer-quote-create-load-balancer", '.', C, C, A),
    golden("loadbalancer-quote-resize-load-balancer", '.', C, C, A),
    golden("loadbalancer-remove-pool-member", '.', C, C, A),
    golden("loadbalancer-resize-credit-synthetic", '.', C, C, C),
    golden("loadbalancer-resize-load-balancer", '.', C, C, A),
    golden("loadbalancer-update-listener", '.', C, C, A),
    golden("loadbalancer-update-policy", '.', C, C, A),
    golden("loadbalancer-update-pool", '.', C, C, A),
    golden("loadbalancer-update-pool-member", '.', C, C, A),
    golden("monitor-create-channel", '.', C, C, C),
    golden("monitor-create-check", '.', C, C, C),
    golden("monitor-create-log-alarm", '.', C, C, C),
    golden("monitor-create-log-project", '.', C, C, C),
    golden(
        "monitor-create-notification-quota",
        '.',
        C,
        QUOTA_WRITE,
        QUOTA_WRITE,
    ),
    golden(
        "monitor-create-notification-quota-sms",
        '.',
        C,
        QUOTA_SMS,
        QUOTA_SMS,
    ),
    golden("monitor-create-synthetic-quota", '.', C, C, C),
    golden("monitor-create-synthetic-quota-preflight", '.', C, C, C),
    golden("monitor-delete-check", '.', C, C, C),
    golden("monitor-delete-log-alarm", '.', C, C, C),
    golden("monitor-delete-log-project", '.', C, C, C),
    golden(
        "monitor-delete-notification-quota-email",
        '.',
        C,
        QUOTA_RENEW,
        QUOTA_RENEW,
    ),
    golden(
        "monitor-delete-notification-quota-sms",
        '.',
        C,
        QUOTA_RENEW,
        QUOTA_RENEW,
    ),
    golden("monitor-delete-synthetic-quota", '.', C, C, C),
    golden("monitor-delete-synthetic-quota-preflight", '.', C, C, C),
    golden("monitor-ensure-synthetic-quota", '.', C, C, C),
    golden("monitor-get-alarm", '.', C, C, C),
    golden("monitor-get-channel", '.', C, C, C),
    golden("monitor-get-check", '.', C, C, C),
    golden("monitor-get-check-alarm", '.', C, C, C),
    golden("monitor-get-log-project", '.', C, C, C),
    golden(
        "monitor-get-notification-quota",
        '.',
        C,
        QUOTA_WRITE,
        QUOTA_WRITE,
    ),
    golden(
        "monitor-get-notification-quota-sms",
        '.',
        C,
        QUOTA_SMS,
        QUOTA_SMS,
    ),
    golden("monitor-get-notification-quota-usage", '.', C, C, C),
    golden("monitor-get-quota-usage", '.', C, C, C),
    golden(
        "monitor-get-synthetic-quota",
        '.',
        C,
        QUOTA_READ,
        QUOTA_READ,
    ),
    golden("monitor-get-synthetic-quota-usage", '.', C, C, C),
    golden("monitor-list-alarms", '.', C, C, C),
    golden("monitor-list-channel-types", '.', C, C, C),
    golden("monitor-list-channels", '.', C, C, C),
    golden("monitor-list-check-alarm-history", '.', C, C, C),
    golden("monitor-list-checks", '.', C, C, C),
    golden("monitor-list-locations", '.', C, C, C),
    golden("monitor-list-log-project-classes", '.', C, C, C),
    golden("monitor-list-log-projects", '.', C, C, C),
    golden(
        "monitor-list-notification-packages",
        '.',
        C,
        QUOTA_READ,
        QUOTA_READ,
    ),
    golden(
        "monitor-list-synthetic-packages",
        '.',
        C,
        QUOTA_COVERAGE,
        QUOTA_COVERAGE,
    ),
    golden("monitor-list-synthetic-packages-query", '.', C, C, C),
    golden("monitor-pause-check", '.', C, C, C),
    golden(
        "monitor-quota-status",
        '.',
        C,
        QUOTA_COVERAGE,
        QUOTA_COVERAGE,
    ),
    golden("monitor-quota-status-query", '.', C, C, C),
    golden("monitor-quote-create-log-project", '.', C, C, C),
    golden("monitor-quote-create-notification-quota", '.', C, C, C),
    golden("monitor-quote-create-notification-quota-sms", '.', C, C, C),
    golden("monitor-quote-create-synthetic-quota", '.', C, C, C),
    golden("monitor-quote-renew-synthetic-quota", '.', C, C, C),
    golden("monitor-recover-synthetic-quota", '.', C, C, C),
    golden("monitor-renew-synthetic-quota", '.', C, C, C),
    golden("monitor-send-channel-otp", '.', C, C, C),
    golden(
        "monitor-set-notification-quota-auto-renew-email",
        '.',
        C,
        QUOTA_RENEW,
        QUOTA_RENEW,
    ),
    golden(
        "monitor-set-notification-quota-auto-renew-sms",
        '.',
        C,
        QUOTA_RENEW,
        QUOTA_RENEW,
    ),
    golden(
        "monitor-set-synthetic-quota-auto-renew-partial",
        '.',
        C,
        C,
        C,
    ),
    golden("monitor-update-channel", '.', C, C, C),
    golden("monitor-update-check", '.', C, C, C),
    golden("monitor-update-log-alarm", '.', C, C, C),
    golden("nested-object", '.', C, C, C),
    golden("network-list-nat-instances", '.', A, C, A),
    golden(
        "network-list-network-prices",
        '.',
        C,
        NETWORK_DISCOVERY,
        NETWORK_DISCOVERY,
    ),
    golden("network-list-vpn-connections", '.', A, C, A),
    golden("nulls", '.', C, C, C),
    golden("numeric-filter", '.', C, C, C),
    golden("paged-list", '.', C, C, C),
    golden("portal-get-quota", '.', C, C, A),
    golden("portal-list-zones", '.', C, C, C),
    golden("pricing-estimate", '.', C, ESTIMATE, ESTIMATE),
    golden("pricing-estimate-partial", '.', C, ESTIMATE, ESTIMATE),
    golden("pricing-estimate-unknown", '.', C, ESTIMATE, ESTIMATE),
    golden("pricing-resize-credit", '.', C, C, C),
    golden("project-list-projects", '.', C, C, C),
    golden("projection", '.', C, C, C),
    golden("storage-get-bucket", '.', C, C, A),
    golden("storage-list-buckets", '.', C, C, A),
    golden("storage-list-storage-prices", '.', C, DISCOVERY, DISCOVERY),
    golden("volume-attach-volume", '.', C, C, A),
    golden("volume-create-volume", '.', C, C, A),
    golden("volume-delete-volume", '.', C, C, A),
    golden("volume-detach-volume", '.', C, C, A),
    golden("volume-get-default-volume-type", '.', C, C, A),
    golden("volume-get-volume", '.', C, C, A),
    golden("volume-list-volume-prices", '.', C, DISCOVERY, DISCOVERY),
    golden("volume-list-volumes", '.', C, C, A),
    golden("volume-list-volumes-by-server", '.', C, C, A),
    golden("volume-quote-create-volume", '.', C, C, A),
    golden("volume-resize-credit-synthetic", '.', C, C, C),
    golden("volume-resize-volume", '.', C, C, A),
];

#[test]
fn inventory_matches_all_golden_files() {
    use std::collections::BTreeSet;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
    let actual: BTreeSet<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".golden"))
        .collect();
    let mut declared = BTreeSet::new();
    for item in INVENTORY {
        for (format, coverage) in [
            ("json", item.json),
            ("table", item.table),
            ("text", item.text),
        ] {
            if let Coverage::Excluded(reason) = coverage {
                assert!(!reason.is_empty());
            }
            if !matches!(coverage, Coverage::Absent) {
                let name = format!("{}{}{}.golden", item.case, item.separator, format);
                assert!(declared.insert(name));
            }
        }
    }
    if let Some(missing) = actual.difference(&declared).next() {
        panic!("unlisted: {missing}");
    }
    if let Some(missing) = declared.difference(&actual).next() {
        panic!("missing: {missing}");
    }
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

fn typed_golden<T: Serialize>(name: &str, value: &T) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/render/golden")
        .join(name);
    let expected = std::fs::read(path).unwrap();
    let mut actual = Vec::new();
    write_json(&mut actual, value).unwrap();
    oracle_equal(name, &actual, &expected);
}

#[test]
fn typed_embedded_golden() {
    struct Base {
        uuid: &'static str,
        name: &'static str,
    }
    struct Embedded {
        base: Base,
        status: &'static str,
    }
    impl Serialize for Embedded {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut object = s.serialize_struct("Embedded", 3)?;
            object.serialize_field("UUID", &self.base.uuid)?;
            object.serialize_field("Name", &self.base.name)?;
            object.serialize_field("Status", &self.status)?;
            object.end()
        }
    }
    typed_golden(
        "embedded-struct.json.golden",
        &Embedded {
            base: Base {
                uuid: "e-1",
                name: "widget",
            },
            status: "ACTIVE",
        },
    );
}

#[test]
fn typed_paged_list_golden() {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Budget {
        #[serde(rename = "UUID")]
        uuid: &'static str,
        name: &'static str,
        limit_amount: i64,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Paged {
        items: Vec<Budget>,
        page: i64,
        page_size: i64,
        total_page: i64,
        total_item: i64,
    }
    let value = Paged {
        items: vec![
            Budget {
                uuid: "b-1",
                name: "monthly",
                limit_amount: 50,
            },
            Budget {
                uuid: "b-2",
                name: "quarterly",
                limit_amount: 150,
            },
        ],
        page: 1,
        page_size: 10,
        total_page: 1,
        total_item: 2,
    };
    typed_golden("paged-list.json.golden", &value);
}

#[test]
fn typed_sorted_map_golden() {
    #[derive(Serialize)]
    #[serde(untagged)]
    enum Scalar {
        Number(i64),
        Text(&'static str),
    }
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Output {
        quota: std::collections::HashMap<&'static str, Scalar>,
    }
    let quota = std::collections::HashMap::from([
        ("used", Scalar::Number(1)),
        ("quotaName", Scalar::Text("vcpu")),
        ("limit", Scalar::Number(10)),
    ]);
    typed_golden("portal-get-quota.json.golden", &Output { quota });
}

#[test]
fn typed_float_golden() {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Summary {
        current_cost: f64,
        forecast_cost: f64,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Point {
        date: &'static str,
        cost: f64,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Output {
        summary: Summary,
        series: Vec<Point>,
        start_date: &'static str,
        end_date: &'static str,
    }
    let value = Output {
        summary: Summary {
            current_cost: 120.5,
            forecast_cost: 200.0,
        },
        series: vec![
            Point {
                date: "2026-01-01",
                cost: 10.25,
            },
            Point {
                date: "2026-01-02",
                cost: 20.0,
            },
        ],
        start_date: "2026-01-01",
        end_date: "2026-01-31",
    };
    typed_golden("nested-object.json.golden", &value);
}

#[test]
fn typed_control_characters_golden() {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Output {
        name: &'static str,
    }
    typed_golden(
        "control-chars.json.golden",
        &Output {
            name: "a\tb\nc\rd\u{001b}E",
        },
    );
}

#[test]
fn json_goldens_round_trip() {
    static GOLDENS: std::sync::OnceLock<Vec<(String, Vec<u8>, serde_json::Value)>> =
        std::sync::OnceLock::new();
    let goldens = GOLDENS.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
        INVENTORY
            .iter()
            .filter_map(|item| {
                if !matches!(item.json, Coverage::Compared) {
                    return None;
                }
                let name = format!("{}{}json.golden", item.case, item.separator);
                let bytes = std::fs::read(dir.join(&name)).unwrap();
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
    assert_eq!(goldens.len(), 153);
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

fn source_for_case(item: &Golden, dir: &Path) -> serde_json::Value {
    if matches!(item.json, Coverage::Compared) {
        let path = dir.join(format!("{}{}json.golden", item.case, item.separator));
        return serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    }
    match item.case {
        "compute-create-server-floating-ip-query" => serde_json::json!("192.0.2.1"),
        "network-list-nat-instances" => {
            serde_json::json!([{"ID":"nat-1", "Name":"edge", "Status":"ACTIVE"}])
        }
        "network-list-vpn-connections" => serde_json::json!([
            {"ID":"vpn-1", "Name":"edge", "Sites":2, "Status":"ACTIVE", "Tunnels":3},
            {"ID":"vpn-2", "Name":"pending", "Sites":0, "Status":"PROVISIONING", "Tunnels":0}
        ]),
        _ => panic!("missing source for {}", item.case),
    }
}

fn compare_format(format: &str) -> usize {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/golden");
    let mut count = 0;
    for item in INVENTORY {
        let coverage = if format == "table" {
            item.table
        } else {
            item.text
        };
        if !matches!(coverage, Coverage::Compared) {
            continue;
        }
        let source = source_for_case(item, &dir);
        let tree = normalize(&source).unwrap();
        let name = format!("{}{}{}.golden", item.case, item.separator, format);
        let expected = std::fs::read(dir.join(&name)).unwrap();
        let mut actual = Vec::new();
        if format == "table" {
            render::write_table(&mut actual, &tree).unwrap();
        } else {
            render::write_text(&mut actual, &tree).unwrap();
        }
        oracle_equal(&name, &actual, &expected);
        count += 1;
    }
    count
}

#[test]
fn text_goldens() {
    assert_eq!(compare_format("text"), 77);
}

#[test]
fn table_goldens() {
    assert_eq!(compare_format("table"), 131);
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
    let mut rows = vec![render::Value::Null; 64];
    rows[0] = render::Value::String("a".repeat(1 << 20));
    let value = render::Value::Array(rows);
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
