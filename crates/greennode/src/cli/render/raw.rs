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
            let key = serde_json::from_str::<String>(key)
                .map_err(|_| Error::Invalid("invalid raw JSON".into()))?;
            object.insert(key, query(value)?);
        }
        return Ok(Value::Object(object));
    }
    match input {
        "null" => Ok(Value::Null),
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        _ if input.starts_with('"') => Ok(Value::String(
            serde_json::from_str(input).map_err(|_| Error::Invalid("invalid raw JSON".into()))?,
        )),
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
    fn raw_json_preserves_order_and_numbers_and_rejects_invalid() {
        let value = project(&RawJson(" { \"z\": 1.00, \"a\": 2 } ")).unwrap();
        let mut out = Vec::new();
        write_json(&mut out, &value).unwrap();
        assert!(out == b"{\"z\":1.00,\"a\":2}\n", "raw bytes differ");
        assert!(project(&RawJson("{]")).is_err());
        let big = project(&RawJson("1e10000")).unwrap();
        assert!(write_json(&mut Vec::new(), &big).is_ok());
        assert!(big.for_query().is_err());
        let allowed = format!("{}null{}", "[".repeat(128), "]".repeat(128));
        let projection = project(&RawJson(&allowed)).unwrap();
        assert!(write_json(&mut Vec::new(), &projection).is_ok());
        assert!(projection.for_query().is_ok());
        let deep = format!("{}null{}", "[".repeat(129), "]".repeat(129));
        assert!(matches!(project(&RawJson(&deep)), Err(Error::DepthLimit)));
    }
}
