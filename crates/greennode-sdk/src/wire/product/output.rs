use super::{
    evidence::{Evidence, identifier_key},
    response::BodyMatcher,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) trait PublicOutput: std::fmt::Debug {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool;
}

pub(crate) struct FieldScan<'a>(BodyMatcher<'a>);

impl<'a> FieldScan<'a> {
    pub(crate) fn new(evidence: &'a Evidence) -> Self {
        Self(BodyMatcher::with_work(evidence.credentials(), 1 << 30))
    }

    pub(crate) fn field<T: PublicOutput + ?Sized>(
        &self,
        value: &T,
        identifier: bool,
        wire: &str,
        output: &str,
    ) -> bool {
        value.public_reflects(
            self,
            identifier || super::identifier::declared(wire, output),
        )
    }

    fn text(&self, text: &str, identifier: bool) -> bool {
        self.0.reflects(text.as_bytes(), identifier)
    }
}

macro_rules! public_fields {
    ($name:ident $(<$item:ident>)? { $($field:ident),* $(; $($private:ident),* $(,)?)? }) => {
        crate::error::debug_record!($name $(<$item>)? { $($field,)* $($($private,)*)? });
        crate::wire::product::output::public_fields!(custom_debug $name $(<$item>)? { $($field),* $(; $($private),*)? });
    };
    (custom_debug $name:ident $(<$item:ident>)? { $($field:ident),* $(; $($private:ident),* $(,)?)? }) => {
        impl $(<$item: crate::wire::product::output::PublicOutput>)? crate::wire::product::output::PublicOutput for $name $(<$item>)? {
            fn public_reflects(&self, _scan: &crate::wire::product::output::FieldScan<'_>, _identifier: bool) -> bool {
                let Self { $($field,)* $($($private: _,)*)? } = self;
                $( _scan.field($field, _identifier, stringify!($field), stringify!($field)) || )* false
            }
        }
    };
}

pub(crate) use public_fields;

impl PublicOutput for str {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        scan.text(self, identifier)
    }
}

impl PublicOutput for String {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        scan.text(self, identifier)
    }
}

impl PublicOutput for crate::Secret {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        scan.text(self.expose_secret(), identifier)
    }
}

impl<T: PublicOutput + ?Sized> PublicOutput for &T {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        (*self).public_reflects(scan, identifier)
    }
}

impl<T: PublicOutput> PublicOutput for Vec<T> {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        self.iter()
            .any(|value| value.public_reflects(scan, identifier))
    }
}

impl<T: PublicOutput> PublicOutput for Option<T> {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        self.as_ref()
            .is_some_and(|value| value.public_reflects(scan, identifier))
    }
}

impl<T: PublicOutput> PublicOutput for BTreeMap<String, T> {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        self.iter().any(|(key, value)| {
            scan.text(key, identifier)
                || value.public_reflects(scan, identifier || identifier_key(key))
        })
    }
}

impl PublicOutput for () {
    fn public_reflects(&self, _: &FieldScan<'_>, _: bool) -> bool {
        false
    }
}

macro_rules! display_values {
    ($($ty:ty),*) => {$(
        impl PublicOutput for $ty {
            fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
                scan.text(&self.to_string(), identifier)
            }
        }
    )*};
}

display_values!(
    bool,
    i32,
    i64,
    u16,
    u32,
    u64,
    usize,
    f32,
    f64,
    serde_json::Number,
    time::OffsetDateTime
);

impl PublicOutput for Value {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        match self {
            Self::Null => scan.text("null", identifier),
            Self::Bool(value) => value.public_reflects(scan, identifier),
            Self::Number(value) => value.public_reflects(scan, identifier),
            Self::String(value) => value.public_reflects(scan, identifier),
            Self::Array(items) => items.public_reflects(scan, identifier),
            Self::Object(fields) => fields.iter().any(|(key, value)| {
                scan.text(key, identifier)
                    || value.public_reflects(scan, identifier || identifier_key(key))
            }),
        }
    }
}

impl<T: PublicOutput + ?Sized> PublicOutput for Box<T> {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        self.as_ref().public_reflects(scan, identifier)
    }
}

impl PublicOutput for serde_json::value::RawValue {
    fn public_reflects(&self, scan: &FieldScan<'_>, identifier: bool) -> bool {
        let body = self.get().as_bytes();
        if identifier {
            super::evidence::identifier_projection_reflects(&scan.0, body) != Some(false)
        } else {
            super::evidence::retained_projection_reflects(&scan.0, body) != Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use regex_lite::Regex;
    use std::{collections::BTreeSet, path::Path};

    #[test]
    fn public_debug_does_not_use_flag_forwarding_derives() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let declarations = Regex::new(r"(?ms)#\[derive\(([^)]*)\)\]\n(?:(?:[ \t]*#\[[^\n]*\]|[ \t]*///[^\n]*)\n)*[ \t]*pub (?:struct|enum) ([\w$]+)").unwrap();
        fn inspect(path: &Path, declarations: &Regex) {
            for entry in path.read_dir().unwrap() {
                let file = entry.unwrap().path();
                if file.file_name().is_some_and(|name| {
                    matches!(
                        name.to_str(),
                        Some("tests" | "tests.rs" | "testing.rs" | "test_server.rs")
                    )
                }) {
                    continue;
                }
                if file.is_dir() {
                    inspect(&file, declarations);
                } else if file.extension().is_some_and(|extension| extension == "rs") {
                    let source = std::fs::read_to_string(&file).unwrap();
                    for declaration in declarations.captures_iter(&source) {
                        assert!(
                            !declaration[1].split(',').any(|name| name.trim() == "Debug"),
                            "{} {}",
                            file.display(),
                            &declaration[2]
                        );
                    }
                }
            }
        }
        inspect(&root, &declarations);
    }

    #[test]
    fn typed_output_registrations_visit_every_public_field() {
        let records =
            Regex::new(r"(?ms)^pub struct (\w+)(?:<T>)?[ \t]*\{(?:[ \t]*\}|[ \t]*\n(.*?)^\})")
                .unwrap();
        let registrations = Regex::new(
            r"(?s)public_fields!\(\s*(?:custom_debug\s+)?(\w+)(?:<T>)?\s*\{([^};]*)(?:;[^}]*)?\}\s*\)",
        )
        .unwrap();
        let public_fields = Regex::new(r"(?m)^    pub ((?:r#)?\w+)\s*:").unwrap();
        fn inspect(
            path: &Path,
            records: &Regex,
            registrations: &Regex,
            public_fields: &Regex,
        ) -> usize {
            let mut checked = 0;
            for file in std::fs::read_dir(path).unwrap() {
                let file = file.unwrap().path();
                if file.is_dir() {
                    checked += inspect(&file, records, registrations, public_fields);
                } else if file.extension().is_some_and(|extension| extension == "rs") {
                    let source = std::fs::read_to_string(&file).unwrap();
                    for registration in registrations.captures_iter(&source) {
                        let name = &registration[1];
                        let target = records
                            .captures_iter(&source)
                            .find(|record| &record[1] == name)
                            .unwrap_or_else(|| {
                                panic!("{} has no output record {name}", file.display())
                            });
                        let expected: BTreeSet<_> = public_fields
                            .captures_iter(target.get(2).map_or("", |body| body.as_str()))
                            .filter_map(|field| field.get(1).map(|name| name.as_str()))
                            .collect();
                        let visited: BTreeSet<_> = registration[2]
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .collect();
                        assert_eq!(visited, expected, "{} {name}", file.display());
                        checked += 1;
                    }
                }
            }
            checked
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        assert!(inspect(&root, &records, &registrations, &public_fields) > 100);
    }
}
