use std::{collections::HashSet, fmt};

use serde::Deserializer;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Error;

pub(crate) enum Shape<'a> {
    Any,
    Object(&'a [ShapeField<'a>]),
    Array(&'a Shape<'a>),
}
pub(crate) struct ShapeField<'a> {
    pub(crate) name: &'a str,
    pub(crate) shape: Shape<'a>,
}
pub(crate) fn validate_shape(body: &[u8], shape: &Shape<'_>, limit: usize) -> Result<(), Error> {
    validate(body, &[], limit)?;
    let raw: &RawValue = serde_json::from_slice(body).map_err(|_| Error)?;
    shape_value(raw, shape)
}
fn shape_value(raw: &RawValue, shape: &Shape<'_>) -> Result<(), Error> {
    if raw.get() == "null" {
        return Ok(());
    }
    match shape {
        Shape::Any => Ok(()),
        Shape::Object(fields) => {
            let values: std::collections::BTreeMap<String, &RawValue> =
                serde_json::from_str(raw.get()).map_err(|_| Error)?;
            let mut assigned = HashSet::new();
            for (name, value) in values {
                let index = fields
                    .iter()
                    .position(|field| field.name == name)
                    .or_else(|| {
                        fields
                            .iter()
                            .position(|field| field_matches(field.name, &name))
                    });
                if let Some(index) = index {
                    if !assigned.insert(index) {
                        return Err(Error);
                    }
                    if let Some(field) = fields.get(index) {
                        shape_value(value, &field.shape)?;
                    }
                }
            }
            Ok(())
        }
        Shape::Array(item) => {
            let values: Vec<&RawValue> = serde_json::from_str(raw.get()).map_err(|_| Error)?;
            for value in values {
                shape_value(value, item)?;
            }
            Ok(())
        }
    }
}

pub(crate) struct Field<'a> {
    pub(crate) name: &'a str,
    pub(crate) children: &'a [Field<'a>],
}

pub(crate) fn validate(body: &[u8], fields: &[Field<'_>], limit: usize) -> Result<(), Error> {
    if body.len() > limit {
        return Err(Error);
    }
    let raw: &RawValue = serde_json::from_slice(body).map_err(|_| Error)?;
    validate_value(raw.get(), fields, 0)
}

pub(crate) fn field_matches(left: &str, right: &str) -> bool {
    fold(left).eq(fold(right))
}

fn fold(value: &str) -> impl Iterator<Item = char> + '_ {
    value.chars().map(|c| match c {
        '\u{017f}' => 's',
        '\u{212a}' => 'k',
        _ => c.to_ascii_lowercase(),
    })
}

fn validate_value(raw: &str, fields: &[Field<'_>], depth: usize) -> Result<(), Error> {
    if depth >= 128 && matches!(raw.as_bytes().first(), Some(b'{' | b'[')) {
        return Err(Error);
    }
    let mut decoder = serde_json::Deserializer::from_str(raw);
    let visitor = Check(fields, depth);
    match raw.as_bytes().first() {
        Some(b'{') => decoder.deserialize_map(visitor).map_err(|_| Error),
        Some(b'[') => decoder.deserialize_seq(visitor).map_err(|_| Error),
        _ => Ok(()),
    }
}

struct Check<'a>(&'a [Field<'a>], usize);

impl<'de> Visitor<'de> for Check<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON container")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut keys = HashSet::new();
        let mut assigned = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate key"));
            }
            let index = self
                .0
                .iter()
                .position(|field| field.name == key)
                .or_else(|| {
                    self.0
                        .iter()
                        .position(|field| fold(field.name).eq(fold(&key)))
                });
            if index.is_some_and(|i| !assigned.insert(i)) {
                return Err(serde::de::Error::custom("duplicate field"));
            }
            let raw = map.next_value::<&RawValue>()?;
            let children = index.map_or(&[][..], |i| self.0[i].children);
            validate_value(raw.get(), children, self.1 + 1)
                .map_err(|_| serde::de::Error::custom("invalid value"))?;
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(raw) = seq.next_element::<&RawValue>()? {
            validate_value(raw.get(), self.0, self.1 + 1)
                .map_err(|_| serde::de::Error::custom("invalid value"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(body: &[u8], names: &[&str]) -> Result<(), Error> {
        let fields: Vec<_> = names
            .iter()
            .map(|name| Field {
                name,
                children: &[],
            })
            .collect();
        validate(body, &fields, 64 * 1024)
    }

    #[test]
    fn validates_nested_container_shapes() {
        let shape = Shape::Object(&[ShapeField {
            name: "items",
            shape: Shape::Array(&Shape::Object(&[])),
        }]);
        assert!(validate_shape(br#"{"items":[{"id":1}]}"#, &shape, 1024).is_ok());
        assert!(validate_shape(br#"{"items":[[1]]}"#, &shape, 1024).is_err());
        assert!(validate_shape(br#"{"items":{}}"#, &shape, 1024).is_err());
    }

    #[test]
    fn duplicate_keys() {
        assert!(check(br#"{"unknown":[{"key":1,"key":2}]}"#, &["id"]).is_err());
        assert!(check(br#"{"unknown":[{"key":1},{"key":2}]}"#, &["id"]).is_ok());
    }

    #[test]
    fn escaped_duplicate() {
        let body = format!(r#"{{"id":"a","{}u0069d":"b"}}"#, char::from(92));
        assert!(check(body.as_bytes(), &["id"]).is_err());
        assert!(check(br#"{"id":"a","other":"b"}"#, &["id"]).is_ok());
    }

    #[test]
    fn known_aliases() {
        assert!(check(br#"{"Id":"a","id":"b"}"#, &["id"]).is_err());
        assert!(check(br#"{"future":{},"FUTURE":{}}"#, &["id"]).is_ok());
        assert!(check(br#"{"id":1,"ID":2}"#, &["id", "ID"]).is_ok());
    }

    #[test]
    fn unicode_aliases() {
        assert!(check("{\"K\":1,\"K\":2}".as_bytes(), &["K"]).is_err());
        assert!(check("{\"S\":1,\"ſ\":2}".as_bytes(), &["S"]).is_err());
        assert!(check("{\"K\":1,\"S\":2}".as_bytes(), &["K", "S"]).is_ok());
    }

    #[test]
    fn nested_schema() {
        let fields = [Field {
            name: "data",
            children: &[Field {
                name: "id",
                children: &[],
            }],
        }];
        assert!(validate(br#"{"data":[{"id":1,"ID":2}]}"#, &fields, 64 * 1024).is_err());
        assert!(validate(br#"{"data":[{"id":1},{"ID":2}]}"#, &fields, 64 * 1024).is_ok());
    }

    #[test]
    fn body_limit() {
        let valid = format!("\"{}\"", "a".repeat(65534));
        let too_large = format!("\"{}\"", "a".repeat(65535));
        assert!(check(valid.as_bytes(), &[]).is_ok());
        assert!(check(too_large.as_bytes(), &[]).is_err());
    }

    #[test]
    fn iam_body_limit() {
        let valid = format!("\"{}\"", "a".repeat(1024 * 1024 - 2));
        let too_large = format!("\"{}\"", "a".repeat(1024 * 1024 - 1));
        assert!(validate(valid.as_bytes(), &[], 1024 * 1024).is_ok());
        assert!(validate(too_large.as_bytes(), &[], 1024 * 1024).is_err());
    }

    #[test]
    fn depth_limit() {
        let body = format!("{}0{}", "[".repeat(129), "]".repeat(129));
        assert!(check(body.as_bytes(), &[]).is_err());
        let body = format!("{}0{}", "[".repeat(20), "]".repeat(20));
        assert!(check(body.as_bytes(), &[]).is_ok());
    }

    #[test]
    fn invalid_syntax() {
        assert!(check(b"{} {}", &[]).is_err());
        assert!(check(br#"{"unknown":1e1000}"#, &[]).is_ok());
    }
}
