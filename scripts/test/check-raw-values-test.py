import runpy
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
GATE = runpy.run_path(str(SCRIPTS / "check-raw-values.py"))
PATH = "greennode-sdk/src/model.rs"


class RawValueAuditTests(unittest.TestCase):
    def test_rejects_unresolved_deserialize_and_raw_value_import_aliases(self):
        source = """
use serde::{Deserialize as Decode, Serialize};
use serde_json::value::RawValue as Raw;
#[derive(Decode)]
struct A(Raw);
"""
        self.assertEqual(GATE["violations"](PATH, source), [2, 3])

    def test_checks_fields_after_parenthesized_where_bounds(self):
        source = """
#[derive(Deserialize)]
struct Model<T> where T: Fn(u64) -> u64 {
    callable: T,
    raw: Box<RawValue>,
}
#[derive(Deserialize)]
enum Reply<T> where T: Fn() { Payload(T, Box<RawValue>) }
"""
        self.assertEqual(GATE["violations"](PATH, source), [5, 8])

    def test_reports_all_unannotated_raw_forms(self):
        source = '''
#[derive(Clone, serde::Deserialize)]
pub(crate) struct Model<'a> {
    a: RawValue,
    pub b: Box<serde_json::value::RawValue>,
    c: &'a RawValue,
    d: Option<Box<RawValue>>,
    e: ::std::option::Option<&'a serde_json::value::RawValue>,
    f: BTreeMap<String, RawValue>,
    g: Option<RawValue>,
}
#[derive(serde::Deserialize)]
struct Newtype(Box<RawValue>);
'''
        self.assertEqual(GATE["violations"](PATH, source), [4, 5, 6, 7, 8, 9, 10, 13])

    def test_checks_named_and_tuple_enum_variants(self):
        source = '''
#[derive(serde::Deserialize)]
enum Reply<'a> {
    Named { raw: &'a RawValue },
    Tuple(Box<RawValue>),
    Unit,
    Checked(#[serde(deserialize_with = "lossy::boxed")] Box<RawValue>),
}
#[derive(Serialize)]
enum Output { Unchecked(Box<RawValue>) }
'''
        self.assertEqual(GATE["violations"](PATH, source), [4, 5])

    def test_checks_nested_generic_field_types(self):
        source = '''
#[derive(Deserialize)]
struct Model<'a> {
    rows: Vec<Box<RawValue>>,
    inventory: Option<HashMap<String, Box<RawValue>>>,
    named: BTreeMap<String, Vec<Option<&'a RawValue>>>,
    other: Box<Vec<u64>>,
}
#[derive(Deserialize)]
enum Reply { Paged(Vec<Option<Box<RawValue>>>) }
'''
        self.assertEqual(GATE["violations"](PATH, source), [4, 5, 6, 10])

    def test_checks_transitive_type_aliases_in_other_modules(self):
        from tempfile import TemporaryDirectory

        with TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates" / "greennode-sdk" / "src"
            crate.mkdir(parents=True)
            (crate / "alias.rs").write_text('''
            type Retained = Box<serde_json::value::RawValue>;
            type Optional = Option<Retained>;
            type Ordinary = Option<String>;
            ''')
            (crate / "model.rs").write_text('''
            #[derive(Deserialize)]
            struct Reply { raw: crate::alias::Optional, other: crate::alias::Ordinary }
            ''')
            self.assertEqual(
                GATE["audit"](root),
                [f"{PATH}:3: deserialize RawValue through wire::product::lossy and use unaliased imports"],
            )

    def test_accepts_shared_helpers_and_non_deserialized_fields(self):
        source = '''
#[derive(Clone, Deserialize)]
#[serde(default)]
struct Model<'a> {
    #[serde(default, deserialize_with = "lossy::present_boxed")]
    a: Option<Box<RawValue>>,
    #[serde(borrow, deserialize_with = "lossy::borrowed")]
    b: &'a RawValue,
}
#[derive(serde::Deserialize)]
#[serde(transparent)]
struct Newtype(
    #[serde(deserialize_with = "crate::wire::product::lossy::boxed")] Box<RawValue>,
);
#[derive(Serialize)]
struct Output { a: Box<RawValue> }
'''
        self.assertEqual(GATE["violations"](PATH, source), [])

    def test_similar_literals_and_other_attributes_do_not_hide_a_field(self):
        source = '''
const TEXT: &str = r#"#[derive(Deserialize)] struct Fake(Box<RawValue>);"#;
#[derive(Deserialize)]
struct Model {
    #[serde(rename = "lossy::boxed", deserialize_with = "other::boxed")]
    a: Box<RawValue>,
    /* #[serde(deserialize_with = "lossy::boxed")] */
    b: Box<RawValue>,
    #[serde(alias = "x, deserialize_with = \\\"lossy::boxed\\\"")]
    c: Box<RawValue>,
}
'''
        self.assertEqual(GATE["violations"](PATH, source), [6, 8, 10])
        self.assertEqual(GATE["violations"](GATE["HELPER"], source), [])


if __name__ == "__main__":
    unittest.main()
