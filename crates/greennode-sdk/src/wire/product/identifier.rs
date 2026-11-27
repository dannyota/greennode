use super::{evidence::SentCredentials, response::BodyMatcher};
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

pub(crate) fn declared(wire: &str, output: &str) -> bool {
    super::evidence::identifier_key(wire)
        || matches!((wire, output), ("associatedNetworks", "VPCIDs"))
}

pub(crate) fn check<T: Serialize + ?Sized, E: serde::ser::Error>(value: &T) -> Result<(), E> {
    if !ACTIVE.with(|slot| slot.borrow().is_some()) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(value).map_err(E::custom)?;
    let reflects = ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return true;
        };
        let matcher = BodyMatcher::with_work(&state.credentials, state.remaining);
        match super::evidence::identifier_projection_reflects(&matcher, &bytes) {
            Some(false) => {
                state.remaining = matcher.remaining();
                false
            }
            _ => true,
        }
    });
    if reflects {
        Err(E::custom("unsafe output identifier"))
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

    #[test]
    fn typed_wire_identifier_fields_have_projected_declarations() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let attributes = Regex::new(r"(?s)#\[serde\((.*?)\)\]").unwrap();
        let wire_name = Regex::new(r#"(?:^|[,\s(])deserialize\s*=\s*"([^"\r\n]+)""#).unwrap();
        let projected_name = Regex::new(r#"(?:^|[,\s(])serialize\s*=\s*"([^"\r\n]+)""#).unwrap();
        let field = Regex::new(r"^\s*pub(?:\([^)]*\))?\s+(\w+)\s*:").unwrap();
        inspect_fields(&root, &|path, source| {
            for attribute in attributes.captures_iter(source) {
                let names = &attribute[1];
                let Some(projected) = projected_name.captures(names) else {
                    continue;
                };
                let output = &projected[1];
                let wire = wire_name.captures(names);
                let implicit = attribute
                    .get(0)
                    .and_then(|attribute| field.captures(&source[attribute.end()..]));
                let name = wire
                    .as_ref()
                    .map(|name| &name[1])
                    .or_else(|| implicit.as_ref().map(|name| &name[1]));
                if let Some(name) = name
                    && declared(name, output)
                {
                    assert!(
                        names.contains("identifier::serialize")
                            || (wire.is_none() && super::super::evidence::identifier_key(output)),
                        "{} {name} -> {output}",
                        path.display()
                    );
                }
            }
        });
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
