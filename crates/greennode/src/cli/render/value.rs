use std::collections::BTreeMap;
use std::io::Write;

use super::{Error, MAX_DEPTH, Node};

#[cfg(test)]
mod tests {
    use super::super::{project, write_json};
    use super::*;

    #[test]
    fn text_streams_large_escaped_cell() {
        #[derive(Default)]
        struct Probe {
            largest: usize,
            bytes: usize,
        }
        impl Write for Probe {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.largest = self.largest.max(data.len());
                self.bytes += data.len();
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let value = Value::String("\0".repeat(32 * 1024 * 1024));
        let mut writer = Probe::default();
        write_text(&mut writer, &value).unwrap();
        assert!(writer.largest <= 4);
        assert_eq!(writer.bytes, 128 * 1024 * 1024 + 1);
    }

    #[test]
    fn query_value_can_be_written_as_sorted_json() {
        let result = Value::Object(BTreeMap::from([
            ("z".into(), Value::Number(2.0)),
            ("a".into(), Value::Number(1.0)),
        ]));
        let mut output = Vec::new();
        write_json(&mut output, &project(&result).unwrap()).unwrap();
        assert!(
            output == b"{\n  \"a\": 1,\n  \"z\": 2\n}\n",
            "query JSON differs"
        );
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(value) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Array(value) => serde::Serialize::serialize(value, serializer),
            Self::Object(value) => serde::Serialize::serialize(value, serializer),
        }
    }
}

pub(super) fn convert(value: &Node) -> Result<Value, Error> {
    match value {
        Node::Null => Ok(Value::Null),
        Node::Bool(v) => Ok(Value::Bool(*v)),
        Node::Number(v) => {
            let n: f64 = v
                .parse()
                .map_err(|_| Error::Invalid("invalid number".into()))?;
            if !n.is_finite() {
                return Err(Error::Invalid("nonfinite float".into()));
            }
            Ok(Value::Number(n))
        }
        Node::Float32(v) => {
            let text = super::float::f32(*v, super::float::Style::Json)?;
            let number = text
                .parse()
                .map_err(|_| Error::Invalid("invalid number".into()))?;
            Ok(Value::Number(number))
        }
        Node::Float64(v) => Ok(Value::Number(*v)),
        Node::Raw(v) => super::raw::query(v),
        Node::String(v) => Ok(Value::String(v.clone())),
        Node::Array(v) => Ok(Value::Array(
            v.iter().map(convert).collect::<Result<_, _>>()?,
        )),
        Node::Object(v) => Ok(Value::Object(
            v.iter()
                .map(|(k, v)| Ok((k.clone(), convert(v)?)))
                .collect::<Result<_, Error>>()?,
        )),
    }
}

pub(super) fn check_depth(value: &Value) -> Result<(), Error> {
    fn visit(value: &Value, depth: usize) -> Result<(), Error> {
        match value {
            Value::Array(items) => {
                let next = depth.checked_add(1).ok_or(Error::DepthLimit)?;
                if next > MAX_DEPTH {
                    return Err(Error::DepthLimit);
                }
                for item in items {
                    visit(item, next)?;
                }
            }
            Value::Object(items) => {
                let next = depth.checked_add(1).ok_or(Error::DepthLimit)?;
                if next > MAX_DEPTH {
                    return Err(Error::DepthLimit);
                }
                for item in items.values() {
                    visit(item, next)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, 0)
}

pub(super) fn rows(value: &Value) -> &[Value] {
    let source = match value {
        Value::Object(map) => match map.get("Items") {
            Some(Value::Array(items)) => return items,
            _ => value,
        },
        _ => value,
    };
    match source {
        Value::Array(items) => items,
        _ => std::slice::from_ref(source),
    }
}

pub(super) fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                out.push_str(&format!("\\x{:02x}", u32::from(c)))
            }
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            _ => out.push(c),
        }
    }
    out
}

fn compact<W: Write>(w: &mut W, value: &Value) -> Result<(), Error> {
    match value {
        Value::Null => write!(w, "null")?,
        Value::Bool(b) => write!(w, "{b}")?,
        Value::Number(n) => write!(w, "{}", super::float::f64(*n, super::float::Style::Json)?)?,
        Value::String(s) => super::string(w, s)?,
        Value::Array(items) => {
            write!(w, "[")?;
            for (i, v) in items.iter().enumerate() {
                if i != 0 {
                    write!(w, ",")?;
                }
                compact(w, v)?;
            }
            write!(w, "]")?;
        }
        Value::Object(map) => {
            write!(w, "{{")?;
            for (i, (k, v)) in map.iter().enumerate() {
                if i != 0 {
                    write!(w, ",")?;
                }
                super::string(w, k)?;
                write!(w, ":")?;
                compact(w, v)?;
            }
            write!(w, "}}")?;
        }
    }
    Ok(())
}

pub(super) fn escape_write<W: Write>(w: &mut W, s: &str) -> Result<(), Error> {
    for c in s.chars() {
        match c {
            '\t' => write!(w, "\\t")?,
            '\n' => write!(w, "\\n")?,
            '\r' => write!(w, "\\r")?,
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => write!(w, "\\x{:02x}", u32::from(c))?,
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => write!(w, "\\u{:04x}", u32::from(c))?,
            _ => write!(w, "{c}")?,
        }
    }
    Ok(())
}

pub(super) fn write_cell<W: Write>(w: &mut W, value: &Value) -> Result<(), Error> {
    match value {
        Value::Null => {}
        Value::Bool(value) => write!(w, "{value}")?,
        Value::Number(value) => write!(
            w,
            "{}",
            super::float::f64(*value, super::float::Style::Fixed)?
        )?,
        Value::String(value) => escape_write(w, value)?,
        _ => compact(w, value)?,
    }
    Ok(())
}

pub(super) fn cell(value: &Value) -> Result<String, Error> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => super::float::f64(*n, super::float::Style::Fixed),
        Value::String(s) => Ok(escape(s)),
        _ => {
            let mut bytes = Vec::new();
            compact(&mut bytes, value)?;
            String::from_utf8(bytes).map_err(|_| Error::Invalid("invalid UTF-8".into()))
        }
    }
}

pub fn write_text<W: Write>(w: &mut W, value: &Value) -> Result<(), Error> {
    check_depth(value)?;
    for row in rows(value) {
        match row {
            Value::Object(map) => {
                for (i, item) in map.values().enumerate() {
                    if i != 0 {
                        write!(w, "\t")?;
                    }
                    write_cell(w, item)?;
                }
            }
            _ => write_cell(w, row)?,
        }
        writeln!(w)?;
    }
    Ok(())
}
