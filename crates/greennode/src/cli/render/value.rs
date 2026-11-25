use std::collections::BTreeMap;
use std::io::Write;

use serde::Serialize;

use super::{Encoder, Error, MAX_DEPTH, Node};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

fn convert(value: Node) -> Result<Value, Error> {
    match value {
        Node::Null => Ok(Value::Null),
        Node::Bool(v) => Ok(Value::Bool(v)),
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
            let text = super::float::f32(v, super::float::Style::Json)?;
            let number = text
                .parse()
                .map_err(|_| Error::Invalid("invalid number".into()))?;
            Ok(Value::Number(number))
        }
        Node::Float64(v) => Ok(Value::Number(v)),
        Node::String(v) => Ok(Value::String(v)),
        Node::Array(v) => Ok(Value::Array(
            v.into_iter().map(convert).collect::<Result<_, _>>()?,
        )),
        Node::Object(v) => Ok(Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, convert(v)?)))
                .collect::<Result<_, Error>>()?,
        )),
    }
}

pub fn normalize<T: ?Sized + Serialize>(value: &T) -> Result<Value, Error> {
    convert(value.serialize(Encoder(0))?)
}

pub(super) fn check_depth(value: &Value) -> Result<(), Error> {
    let mut pending = vec![(value, 0_usize)];
    while let Some((item, depth)) = pending.pop() {
        match item {
            Value::Array(items) => {
                let next = depth.checked_add(1).ok_or(Error::DepthLimit)?;
                if next > MAX_DEPTH {
                    return Err(Error::DepthLimit);
                }
                pending.extend(items.iter().map(|v| (v, next)));
            }
            Value::Object(items) => {
                let next = depth.checked_add(1).ok_or(Error::DepthLimit)?;
                if next > MAX_DEPTH {
                    return Err(Error::DepthLimit);
                }
                pending.extend(items.values().map(|v| (v, next)));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn rows(value: &Value) -> Vec<&Value> {
    let source = match value {
        Value::Object(map) => match map.get("Items") {
            Some(Value::Array(items)) => return items.iter().collect(),
            _ => value,
        },
        _ => value,
    };
    match source {
        Value::Array(items) => items.iter().collect(),
        _ => vec![source],
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
                    write!(w, "{}", cell(item)?)?;
                }
            }
            _ => write!(w, "{}", cell(row)?)?,
        }
        writeln!(w)?;
    }
    Ok(())
}
