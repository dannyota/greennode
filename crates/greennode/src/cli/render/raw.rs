use serde::Serialize;

use super::{Error, MAX_DEPTH, Value};
use std::collections::BTreeMap;

pub(super) const MARKER: &str = "$greennode::raw";

pub struct RawJson<'a>(pub &'a str);

impl Serialize for RawJson<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(MARKER, self.0)
    }
}

fn split_top(input: &str, separator: char) -> Result<Vec<&str>, Error> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, c) in input.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else {
            match c {
                '"' => quoted = true,
                '[' | '{' => depth = depth.checked_add(1).ok_or(Error::DepthLimit)?,
                ']' | '}' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?
                }
                _ if c == separator && depth == 0 => {
                    parts.push(
                        input
                            .get(start..index)
                            .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?,
                    );
                    start = index
                        .checked_add(c.len_utf8())
                        .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?;
                }
                _ => {}
            }
        }
    }
    parts.push(
        input
            .get(start..)
            .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?,
    );
    Ok(parts)
}

fn hex_quad<I: Iterator<Item = char>>(input: &mut I) -> Result<u16, Error> {
    let mut value = 0_u16;
    for _ in 0..4 {
        let digit = input
            .next()
            .filter(char::is_ascii)
            .ok_or_else(|| Error::Invalid("invalid raw JSON escape".into()))?
            as u8;
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return Err(Error::Invalid("invalid raw JSON escape".into())),
        };
        value = value * 16 + u16::from(digit);
    }
    Ok(value)
}

pub(crate) fn decode_string(input: &str) -> Result<String, Error> {
    use std::fmt::Write;
    let mut chars = input.chars();
    let mut normalized = String::with_capacity(input.len());
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            normalized.push(ch);
            continue;
        }
        let escape = chars
            .next()
            .ok_or_else(|| Error::Invalid("invalid raw JSON escape".into()))?;
        if escape != 'u' {
            normalized.push('\\');
            normalized.push(escape);
            continue;
        }
        let value = hex_quad(&mut chars)?;
        if (0xd800..=0xdbff).contains(&value) {
            let mut lookahead = chars.clone();
            let low = if lookahead.next() == Some('\\') && lookahead.next() == Some('u') {
                hex_quad(&mut lookahead).ok()
            } else {
                None
            };
            if let Some(low) = low.filter(|n| (0xdc00..=0xdfff).contains(n)) {
                chars = lookahead;
                write!(normalized, "\\u{value:04x}\\u{low:04x}")
                    .map_err(|_| Error::Invalid("invalid raw JSON escape".into()))?;
            } else {
                normalized.push_str("\\ufffd");
            }
        } else if (0xdc00..=0xdfff).contains(&value) {
            normalized.push_str("\\ufffd");
        } else {
            write!(normalized, "\\u{value:04x}")
                .map_err(|_| Error::Invalid("invalid raw JSON escape".into()))?;
        }
    }
    serde_json::from_str(&normalized).map_err(|_| Error::Invalid("invalid raw JSON".into()))
}

pub(super) fn query(input: &str) -> Result<Value, Error> {
    if let Some(inner) = input.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(Value::Array(
            split_top(inner, ',')?
                .into_iter()
                .map(query)
                .collect::<Result<_, _>>()?,
        ));
    }
    if let Some(inner) = input.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut object = BTreeMap::new();
        for entry in split_top(inner, ',')? {
            let mut parts = split_top(entry, ':')?.into_iter();
            let key = parts
                .next()
                .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?;
            let value = parts
                .next()
                .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?;
            if parts.next().is_some() {
                return Err(Error::Invalid("invalid raw JSON".into()));
            }
            let key = decode_string(key)?;
            object.insert(key, query(value)?);
        }
        return Ok(Value::Object(object));
    }
    match input {
        "null" => Ok(Value::Null),
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        _ if input.starts_with('"') => Ok(Value::String(decode_string(input)?)),
        _ => {
            let number: f64 = input
                .parse()
                .map_err(|_| Error::Invalid("invalid raw JSON number".into()))?;
            if !number.is_finite() {
                return Err(Error::Invalid("nonfinite raw JSON number".into()));
            }
            Ok(Value::Number(number))
        }
    }
}

pub(super) fn validate(raw: &str, outer: usize) -> Result<String, Error> {
    let mut nesting = outer;
    let mut quoted = false;
    let mut escaped = false;
    for c in raw.chars() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else {
            match c {
                '"' => quoted = true,
                '{' | '[' => {
                    nesting = nesting.checked_add(1).ok_or(Error::DepthLimit)?;
                    if nesting > MAX_DEPTH {
                        return Err(Error::DepthLimit);
                    }
                }
                '}' | ']' => {
                    nesting = nesting
                        .checked_sub(1)
                        .ok_or_else(|| Error::Invalid("invalid raw JSON".into()))?;
                }
                _ => {}
            }
        }
    }
    serde_json::from_str::<serde::de::IgnoredAny>(raw)
        .map_err(|_| Error::Invalid("invalid raw JSON".into()))?;
    let mut out = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut escaped = false;
    for c in raw.chars() {
        if quoted {
            if escaped {
                escaped = false;
                out.push(c);
            } else if c == '\\' {
                escaped = true;
                out.push(c);
            } else if c == '"' {
                quoted = false;
                out.push(c);
            } else if matches!(c, '<' | '>' | '&' | '\u{007f}'..='\u{009f}' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                use std::fmt::Write;
                write!(out, "\\u{:04x}", u32::from(c))
                    .map_err(|_| Error::Invalid("invalid raw JSON".into()))?;
            } else {
                out.push(c);
            }
        } else if c == '"' {
            quoted = true;
            out.push(c);
        } else if !matches!(c, ' ' | '\t' | '\n' | '\r') {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::{project, write_json};
    use super::*;

    #[test]
    fn raw_marker_charges_depth_and_rejects_nested_markers() {
        struct Wrapped(usize);
        impl Serialize for Wrapped {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                if self.0 == 0 {
                    return RawJson("null").serialize(serializer);
                }
                let mut seq = serializer.serialize_seq(Some(1))?;
                seq.serialize_element(&Wrapped(self.0 - 1))?;
                seq.end()
            }
        }
        struct Nested;
        impl Serialize for Nested {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_newtype_struct(MARKER, &RawJson("null"))
            }
        }
        assert!(project(&Wrapped(127)).is_ok());
        assert!(matches!(project(&Wrapped(128)), Err(Error::DepthLimit)));
        assert!(project(&Nested).is_err());
    }

    #[test]
    fn string_escape_requires_four_hex_digits() {
        assert!(decode_string(r#""\u+123""#).is_err());
        assert!(decode_string(r#""ģ""#).is_ok());
    }

    #[test]
    fn query_replaces_unpaired_surrogates() {
        let scalar = project(&RawJson(r#""\ud800""#)).unwrap();
        assert_eq!(
            scalar.for_query().unwrap(),
            Value::String("\u{fffd}".into())
        );
        let input = concat!(
            r#"{"\ud800":"\udc00","pair":""#,
            r"\ud83d",
            r"\ude00",
            r#""}"#
        );
        let object = project(&RawJson(input)).unwrap();
        let mut output = Vec::new();
        write_json(&mut output, &object).unwrap();
        assert!(output == format!("{input}\n").as_bytes());
        let Value::Object(map) = object.for_query().unwrap() else {
            panic!("expected object");
        };
        assert_eq!(map.get("\u{fffd}"), Some(&Value::String("\u{fffd}".into())));
        assert_eq!(map.get("pair"), Some(&Value::String("😀".into())));
        assert!(project(&RawJson(r#""\q""#)).is_err());
    }

    #[test]
    fn raw_json_preserves_order_and_numbers_and_rejects_invalid() {
        let value = project(&RawJson(" { \"z\": 1.00, \"a\": 2 } ")).unwrap();
        let mut out = Vec::new();
        write_json(&mut out, &value).unwrap();
        assert!(out == b"{\"z\":1.00,\"a\":2}\n", "raw bytes differ");
        assert!(project(&RawJson("{]")).is_err());
        let big = project(&RawJson("1e10000")).unwrap();
        assert!(write_json(&mut Vec::new(), &big).is_ok());
        assert!(big.for_query().is_err());
        let allowed = format!("{}null{}", "[".repeat(127), "]".repeat(127));
        let projection = project(&RawJson(&allowed)).unwrap();
        assert!(write_json(&mut Vec::new(), &projection).is_ok());
        assert!(projection.for_query().is_ok());
        let deep = format!("{}null{}", "[".repeat(128), "]".repeat(128));
        assert!(matches!(project(&RawJson(&deep)), Err(Error::DepthLimit)));
    }
}
