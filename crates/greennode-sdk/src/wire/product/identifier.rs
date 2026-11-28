use super::{
    evidence::SentCredentials,
    response::{BodyMatcher, Screen},
};
use serde::{Serialize, Serializer};
use std::cell::RefCell;

const MATCH_WORK: usize = 64 << 20;

struct Scan {
    credentials: SentCredentials,
    remaining: usize,
}

thread_local! {
    static ACTIVE: RefCell<Option<Scan>> = const { RefCell::new(None) };
}

struct Restore(Option<Scan>);

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

pub(super) fn checking<T>(credentials: &SentCredentials, serialize: impl FnOnce() -> T) -> T {
    let previous = ACTIVE.with(|slot| {
        slot.borrow_mut().replace(Scan {
            credentials: credentials.clone(),
            remaining: MATCH_WORK,
        })
    });
    let _restore = Restore(previous);
    serialize()
}

pub(super) fn declared_wire(wire: &str) -> bool {
    super::evidence::identifier_key(wire)
        || matches!(
            wire,
            "serverIdList"
                | "ServerIDList"
                | "server_id_list"
                | "policyIdList"
                | "associatedNetworks"
                | "subnetAssociationList"
        )
}

pub(crate) fn declared(wire: &str, output: &str) -> bool {
    declared_wire(wire) || declared_wire(output)
}

pub(crate) fn check<T: Serialize + ?Sized, E: serde::ser::Error>(value: &T) -> Result<(), E> {
    check_with(value, super::evidence::identifier_projection_reflects)
}

pub(crate) fn check_retained<T: Serialize + ?Sized, E: serde::ser::Error>(
    value: &T,
) -> Result<(), E> {
    check_with(value, super::evidence::retained_projection_reflects)
}

fn check_with<T: Serialize + ?Sized, E: serde::ser::Error>(
    value: &T,
    reflects: fn(&BodyMatcher<'_>, &[u8]) -> Screen,
) -> Result<(), E> {
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return Ok(());
    }
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(E::custom)?);
    let reflects = ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return true;
        };
        let matcher = BodyMatcher::with_work(&state.credentials, state.remaining);
        match reflects(&matcher, &bytes) {
            screen if screen.is_clean() => {
                state.remaining = matcher.remaining();
                false
            }
            _ => true,
        }
    });
    if reflects {
        Err(E::custom("unsafe output field"))
    } else {
        Ok(())
    }
}

pub(crate) fn serialize<T: Serialize + ?Sized, S: Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    check::<T, S::Error>(value)?;
    value.serialize(serializer)
}

pub(crate) fn checked<T: Serialize + ?Sized>(value: &T) -> impl Serialize + '_ {
    struct Checked<'a, T: ?Sized>(&'a T);

    impl<T: Serialize + ?Sized> Serialize for Checked<'_, T> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serialize(self.0, serializer)
        }
    }

    Checked(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_lite::Regex;
    use std::path::Path;

    fn inspect_fields(path: &Path, inspect: &impl Fn(&Path, &str)) {
        for entry in std::fs::read_dir(path).unwrap() {
            let file = entry.unwrap().path();
            if file.is_dir() {
                inspect_fields(&file, inspect);
            } else if file.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
                inspect(&file, &std::fs::read_to_string(&file).unwrap());
            }
        }
    }

    struct Inventory {
        structs: Regex,
        fields: Regex,
        serde: Regex,
        rename: Regex,
        rename_all: Regex,
        manual: Regex,
        member: Regex,
    }

    impl Inventory {
        fn new() -> Self {
            Self {
                structs: Regex::new(concat!(
                    r"(?ms)^((?:#\[[^\]]*\]\s*|///[^\n]*\n)*)",
                    r"(?:pub(?:\([^)]*\))?\s+)?struct (\w+)[^\n{;]*\{[ \t]*\n(.*?)^}"
                ))
                .unwrap(),
                fields: Regex::new(concat!(
                    r"(?ms)^((?:    (?:#\[[^\]]*\]|///[^\n]*)[ \t]*\n)*)",
                    r"    (?:pub(?:\([^)]*\))?\s+)?(?:r#)?(\w+)\s*:"
                ))
                .unwrap(),
                serde: Regex::new(r"(?s)#\[serde\((.*?)\)\]").unwrap(),
                rename: renames("rename"),
                rename_all: renames("rename_all"),
                manual: Regex::new(r#"(?s)\bserialize_(?:field|entry)\(\s*"(\w+)",\s*(.*?)\)\?"#)
                    .unwrap(),
                member: Regex::new(r"^&(?:\w+\.)+(\w+)$").unwrap(),
            }
        }

        fn serde_attrs(&self, attrs: &str) -> String {
            self.serde
                .captures_iter(attrs)
                .map(|item| item[1].to_owned())
                .collect::<Vec<_>>()
                .join(",")
        }

        fn missing_serde(&self, source: &str) -> Vec<String> {
            use super::super::evidence::identifier_key;

            if !source.contains("Serialize") {
                return Vec::new();
            }
            let mut missing = Vec::new();
            for model in self.structs.captures_iter(source) {
                if !model[1].contains("Serialize") {
                    continue;
                }
                let model_attrs = self.serde_attrs(&model[1]);
                let (wire_case, output_case) = names(&self.rename_all, &model_attrs);
                for field in self.fields.captures_iter(&model[3]) {
                    let attrs = self.serde_attrs(&field[1]);
                    let name = &field[2];
                    let (wire, output) = names(&self.rename, &attrs);
                    let explicit_wire = wire.is_some();
                    let wire = wire.map_or_else(|| field_case(name, wire_case), str::to_owned);
                    let output =
                        output.map_or_else(|| field_case(name, output_case), str::to_owned);
                    if declared(&wire, &output)
                        && (!identifier_key(&output) || (explicit_wire && wire != output))
                        && !attrs.contains("identifier::serialize")
                        && !attrs
                            .split(',')
                            .any(|attr| matches!(attr.trim(), "skip" | "skip_serializing"))
                    {
                        missing.push(format!("{}.{name} ({wire} -> {output})", &model[2]));
                    }
                }
            }
            missing
        }

        fn missing_manual(&self, source: &str) -> Vec<String> {
            if !source.contains("serialize_field(") && !source.contains("serialize_entry(") {
                return Vec::new();
            }
            self.manual
                .captures_iter(source)
                .filter_map(|field| {
                    let output = &field[1];
                    let value = field[2].trim().trim_end_matches(',').trim_end();
                    let wire = self.member.captures(value);
                    let identifier = declared(output, output)
                        || wire.as_ref().is_some_and(|wire| declared(&wire[1], output));
                    (identifier && !value.starts_with("&identifier::checked("))
                        .then(|| output.to_owned())
                })
                .collect()
        }
    }

    fn renames(attribute: &str) -> Regex {
        Regex::new(&format!(r#"\b{attribute}\s*(?:=\s*"([^"]+)"|\(([^)]*)\))"#)).unwrap()
    }

    fn names<'a>(attribute: &Regex, attrs: &'a str) -> (Option<&'a str>, Option<&'a str>) {
        let Some(rename) = attribute.captures(attrs) else {
            return (None, None);
        };
        if let Some(common) = rename.get(1) {
            return (Some(common.as_str()), Some(common.as_str()));
        }
        let directions = rename.get(2).unwrap().as_str();
        let direction = |name: &str| {
            directions
                .split(',')
                .filter_map(|entry| entry.split_once('='))
                .find_map(|(key, value)| {
                    (key.trim() == name).then(|| value.trim().trim_matches('"'))
                })
        };
        (direction("deserialize"), direction("serialize"))
    }

    fn field_case(field: &str, style: Option<&str>) -> String {
        let pascal = || {
            field
                .split('_')
                .filter(|word| !word.is_empty())
                .map(|word| word[..1].to_ascii_uppercase() + &word[1..])
                .collect::<String>()
        };
        match style {
            None | Some("lowercase" | "snake_case") => field.to_owned(),
            Some("UPPERCASE" | "SCREAMING_SNAKE_CASE") => field.to_ascii_uppercase(),
            Some("PascalCase") => pascal(),
            Some("camelCase") => {
                let mut name = pascal();
                name[..1].make_ascii_lowercase();
                name
            }
            Some("kebab-case") => field.replace('_', "-"),
            Some("SCREAMING-KEBAB-CASE") => field.to_ascii_uppercase().replace('_', "-"),
            Some(style) => panic!("unknown Serde rename rule {style}"),
        }
    }

    #[test]
    fn identifier_inventory_catches_effective_serde_names_and_manual_projections() {
        let inventory = Inventory::new();
        let source = r#"
#[derive(Serialize, Deserialize)]
#[serde(rename_all(deserialize = "camelCase", serialize = "PascalCase"))]
struct Projection {
    #[serde(rename(serialize = "VPCID"))]
    vpc_id: String,
    #[serde(rename(serialize = "Project"))]
    project_uuid: String,
    project_id: String,
}
        "#;
        assert_eq!(
            inventory.missing_serde(source),
            [
                "Projection.vpc_id (vpcId -> VPCID)",
                "Projection.project_uuid (projectUuid -> Project)",
            ]
        );
        let serializer = r#"
            out.serialize_field("VPCID", &self.vpc_id)?;
            out.serialize_field("Project", &self.project_uuid)?;
            out.serialize_field("VPCID", &identifier::checked(&self.vpc_id))?;
            out.serialize_entry(key, value)?;
        "#;
        assert_eq!(inventory.missing_manual(serializer), ["VPCID", "Project"]);
    }

    #[test]
    fn guard_receipts_recheck_the_role_of_each_wire_identifier_declaration() {
        use super::super::evidence::Evidence;
        use crate::Secret;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let fields = Regex::new(r#"(?m)^\s*(?P<spec>[^\n]*?)\s*(?P<syntax>=>|=)\s*"(?P<first>\w+)",\s*"(?P<second>\w+)"(?:,\s*[^;\n]*)?;"#).unwrap();
        let checked = std::cell::Cell::new(0);
        inspect_fields(&root, &|path, source| {
            let relative = path.strip_prefix(&root).unwrap();
            if relative.starts_with("wire") || relative.iter().any(|part| part == "tests") {
                return;
            }
            if !source
                .match_indices("\",")
                .any(|(index, pair)| source[index + pair.len()..].trim_start().starts_with('"'))
            {
                return;
            }
            for row in fields.captures_iter(source) {
                let (wire, output) = if &row["syntax"] == "=" && relative.starts_with("volume") {
                    (&row["second"], &row["first"])
                } else if &row["syntax"] == "=>" {
                    (&row["first"], &row["second"])
                } else {
                    continue;
                };
                let explicit = row["spec"].contains("@ screen_id");
                if !declared(wire, output) && !explicit {
                    continue;
                }
                if explicit && !declared_wire(wire) {
                    assert_eq!(relative, Path::new("iam/models.rs"));
                    assert!(source.contains("identifier_role!($($screen)?)"));
                    let fetch = std::fs::read_to_string(root.join("iam/mod.rs")).unwrap();
                    assert!(fetch.contains(
                        "keep_decoded_operands(operation.id, &wire::WireOperands(&output))"
                    ));
                    continue;
                }
                let body = serde_json::json!({wire: ["pre6A6B6Cpost"]}).to_string();
                let mut evidence = Evidence::for_write();
                assert!(evidence.keep_guard_for_test("guard", body.as_bytes()));
                evidence.add_bearer(&Secret::new("jkl"));
                assert!(
                    evidence.recheck_read_proofs().is_err(),
                    "{} {wire} {output}",
                    relative.display()
                );
                checked.set(checked.get() + 1);
            }
        });
        assert!(checked.get() > 100);
    }

    #[test]
    fn typed_wire_identifier_fields_have_projected_declarations() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let inventory = Inventory::new();
        let missing = RefCell::new(Vec::new());
        inspect_fields(&root, &|path, source| {
            if path == root.join("wire/product/identifier.rs") {
                return;
            }
            for field in inventory
                .missing_serde(source)
                .into_iter()
                .chain(inventory.missing_manual(source))
            {
                missing.borrow_mut().push(format!(
                    "{} {field}",
                    path.strip_prefix(&root).unwrap().display()
                ));
            }
        });
        let mut missing = missing.into_inner();
        missing.sort();
        assert!(missing.is_empty(), "{}", missing.join("\n"));
        for path in [
            "model/wire.rs",
            "globalloadbalancer/models.rs",
            "volume/model.rs",
            "iam/models.rs",
        ] {
            let source = std::fs::read_to_string(root.join(path)).unwrap();
            assert!(
                source.contains("identifier::declared($wire,")
                    && source.contains("identifier::check::<"),
                "{path}"
            );
        }
    }
}
