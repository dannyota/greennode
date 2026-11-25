mod float;
mod number;
mod raw;
mod redaction;
pub use raw::RawJson;
mod table;
mod value;
pub use redaction::DynamicMap;
pub use table::write_table;
pub use value::{Value, write_text};

use std::fmt;
use std::io::{self, Write};

use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    TableLimit,
    DepthLimit,
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::DepthLimit => f.write_str("nesting exceeds 128 levels"),
            Self::TableLimit => f.write_str(
                "table output exceeds 100 columns, 1000000 cells, or 64 MiB; use --output json",
            ),
            Self::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Invalid(msg.to_string())
    }
}

enum Node {
    Null,
    Bool(bool),
    Number(String),
    Float32(f32),
    Float64(f64),
    Raw(String),
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

struct ObjectKey {
    name: String,
    redacted: bool,
}

impl ObjectKey {
    fn new(name: String, dynamic: bool) -> Self {
        let redacted = dynamic && redaction::sensitive(&name);
        Self { name, redacted }
    }

    fn finish<F: FnOnce() -> Result<Node, Error>>(self, value: F) -> Result<(String, Node), Error> {
        let node = if self.redacted {
            Node::String("<redacted>".into())
        } else {
            value()?
        };
        Ok((self.name, node))
    }
}

const MAX_DEPTH: usize = 128;

struct Encoder(usize, bool);

impl Encoder {
    fn descend(&self) -> Result<usize, Error> {
        let depth = self.0.checked_add(1).ok_or(Error::DepthLimit)?;
        if depth > MAX_DEPTH {
            return Err(Error::DepthLimit);
        }
        Ok(depth)
    }
}

impl ser::Serializer for Encoder {
    type Ok = Node;
    type Error = Error;
    type SerializeSeq = Seq;
    type SerializeTuple = Seq;
    type SerializeTupleStruct = Seq;
    type SerializeTupleVariant = VariantSeq;
    type SerializeMap = Map;
    type SerializeStruct = Map;
    type SerializeStructVariant = VariantMap;
    fn serialize_bool(self, v: bool) -> Result<Node, Error> {
        Ok(Node::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Node, Error> {
        self.serialize_i64(v.into())
    }
    fn serialize_i16(self, v: i16) -> Result<Node, Error> {
        self.serialize_i64(v.into())
    }
    fn serialize_i32(self, v: i32) -> Result<Node, Error> {
        self.serialize_i64(v.into())
    }
    fn serialize_i64(self, v: i64) -> Result<Node, Error> {
        Ok(Node::Number(v.to_string()))
    }
    fn serialize_i128(self, v: i128) -> Result<Node, Error> {
        Ok(Node::Number(v.to_string()))
    }
    fn serialize_u8(self, v: u8) -> Result<Node, Error> {
        self.serialize_u64(v.into())
    }
    fn serialize_u16(self, v: u16) -> Result<Node, Error> {
        self.serialize_u64(v.into())
    }
    fn serialize_u32(self, v: u32) -> Result<Node, Error> {
        self.serialize_u64(v.into())
    }
    fn serialize_u64(self, v: u64) -> Result<Node, Error> {
        Ok(Node::Number(v.to_string()))
    }
    fn serialize_u128(self, v: u128) -> Result<Node, Error> {
        Ok(Node::Number(v.to_string()))
    }
    fn serialize_f32(self, v: f32) -> Result<Node, Error> {
        if !v.is_finite() {
            return Err(Error::Invalid("nonfinite float".into()));
        }
        Ok(Node::Float32(v))
    }
    fn serialize_f64(self, v: f64) -> Result<Node, Error> {
        if !v.is_finite() {
            return Err(Error::Invalid("nonfinite float".into()));
        }
        Ok(Node::Float64(v))
    }
    fn serialize_char(self, v: char) -> Result<Node, Error> {
        self.serialize_str(&v.to_string())
    }
    fn serialize_str(self, v: &str) -> Result<Node, Error> {
        Ok(Node::String(v.into()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Node, Error> {
        self.descend()?;
        Ok(Node::Array(
            v.iter().map(|n| Node::Number(n.to_string())).collect(),
        ))
    }
    fn serialize_none(self) -> Result<Node, Error> {
        Ok(Node::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Node, Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<Node, Error> {
        Ok(Node::Null)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Node, Error> {
        Ok(Node::Null)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        v: &'static str,
    ) -> Result<Node, Error> {
        self.serialize_str(v)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<Node, Error> {
        if name == redaction::MARKER {
            let map = v.serialize(Encoder(self.descend()?, true))?;
            if !matches!(map, Node::Object(_)) {
                return Err(Error::Invalid("dynamic value must be a map".into()));
            }
            return Ok(map);
        }
        if name == raw::MARKER {
            if self.1 {
                return Err(Error::Invalid("raw JSON inside a dynamic map".into()));
            }
            let depth = self.descend()?;
            return match v.serialize(Encoder(depth, false))? {
                Node::String(raw) => Ok(Node::Raw(raw::validate(&raw, depth)?)),
                _ => Err(Error::Invalid("invalid raw JSON".into())),
            };
        }
        v.serialize(Encoder(self.descend()?, self.1))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Node, Error> {
        let depth = self.descend()?;
        let key = ObjectKey::new(variant.into(), self.1);
        Ok(Node::Object(vec![
            key.finish(|| value.serialize(Encoder(depth, self.1)))?,
        ]))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?, self.1))
    }
    fn serialize_tuple(self, _: usize) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?, self.1))
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?, self.1))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<VariantSeq, Error> {
        Ok(VariantSeq(
            ObjectKey::new(variant.into(), self.1),
            Vec::new(),
            Encoder(self.descend()?, self.1).descend()?,
            self.1,
        ))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Map, Error> {
        Ok(Map {
            fields: Vec::new(),
            key: None,
            sort: true,
            number: false,
            depth: self.descend()?,
            dynamic: self.1,
        })
    }
    fn serialize_struct(self, name: &'static str, _: usize) -> Result<Map, Error> {
        Ok(Map {
            fields: Vec::new(),
            key: None,
            sort: false,
            number: name == "$serde_json::private::Number",
            dynamic: self.1,
            depth: if name == "$serde_json::private::Number" {
                self.0
            } else {
                self.descend()?
            },
        })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<VariantMap, Error> {
        Ok(VariantMap(
            ObjectKey::new(variant.into(), self.1),
            Vec::new(),
            Encoder(self.descend()?, self.1).descend()?,
            self.1,
        ))
    }
}

struct Seq(Vec<Node>, usize, bool);
impl SerializeSeq for Seq {
    type Ok = Node;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        self.0.push(v.serialize(Encoder(self.1, self.2))?);
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        Ok(Node::Array(self.0))
    }
}
impl SerializeTuple for Seq {
    type Ok = Node;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Node, Error> {
        SerializeSeq::end(self)
    }
}
impl SerializeTupleStruct for Seq {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Node, Error> {
        SerializeSeq::end(self)
    }
}
struct VariantSeq(ObjectKey, Vec<Node>, usize, bool);
impl SerializeTupleVariant for VariantSeq {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        if !self.0.redacted {
            self.1.push(v.serialize(Encoder(self.2, self.3))?);
        }
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        Ok(Node::Object(vec![
            self.0.finish(|| Ok(Node::Array(self.1)))?,
        ]))
    }
}
struct Map {
    fields: Vec<(String, Node)>,
    key: Option<ObjectKey>,
    sort: bool,
    number: bool,
    depth: usize,
    dynamic: bool,
}
impl SerializeMap for Map {
    type Ok = Node;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        if self.key.is_some() {
            return Err(Error::Invalid("missing map value".into()));
        }
        let key = match key.serialize(Encoder(self.depth, false))? {
            Node::String(s) | Node::Number(s) => s,
            Node::Float32(n) => float::f32(n, float::Style::Key)?,
            Node::Float64(n) => float::f64(n, float::Style::Key)?,
            Node::Bool(b) => b.to_string(),
            _ => return Err(Error::Invalid("invalid map key".into())),
        };
        self.key = Some(ObjectKey::new(key, self.dynamic));
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error::Invalid("missing map key".into()))?;
        self.fields
            .push(key.finish(|| v.serialize(Encoder(self.depth, self.dynamic)))?);
        Ok(())
    }
    fn end(mut self) -> Result<Node, Error> {
        if self.key.is_some() {
            return Err(Error::Invalid("missing map key".into()));
        }
        if self.number {
            if self.fields.len() == 1
                && let Some((key, Node::String(s))) = self.fields.pop()
                && key == "$serde_json::private::Number"
                && serde_json::from_str::<serde_json::Number>(&s).is_ok()
            {
                return Ok(Node::Number(s));
            }
            return Err(Error::Invalid("invalid number".into()));
        }
        if self.sort {
            self.fields.sort_by(|a, b| a.0.cmp(&b.0));
        }
        Ok(Node::Object(self.fields))
    }
}
impl SerializeStruct for Map {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        if self.number {
            if key != "$serde_json::private::Number" || !self.fields.is_empty() {
                return Err(Error::Invalid("invalid number token".into()));
            }
            self.fields.push((key.into(), v.serialize(number::Token)?));
            return Ok(());
        }
        let key = ObjectKey::new(key.into(), self.dynamic);
        self.fields
            .push(key.finish(|| v.serialize(Encoder(self.depth, self.dynamic)))?);
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        SerializeMap::end(self)
    }
}
struct VariantMap(ObjectKey, Vec<(String, Node)>, usize, bool);
impl SerializeStructVariant for VariantMap {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        if !self.0.redacted {
            let key = ObjectKey::new(key.into(), self.3);
            self.1
                .push(key.finish(|| v.serialize(Encoder(self.2, self.3)))?);
        }
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        Ok(Node::Object(vec![
            self.0.finish(|| Ok(Node::Object(self.1)))?,
        ]))
    }
}

fn string<W: Write>(w: &mut W, s: &str) -> Result<(), Error> {
    write!(w, "\"")?;
    for c in s.chars() {
        match c {
            '"' => write!(w, "\\\"")?,
            '\\' => write!(w, "\\\\")?,
            '\n' => write!(w, "\\n")?,
            '\r' => write!(w, "\\r")?,
            '\t' => write!(w, "\\t")?,
            '\u{08}' => write!(w, "\\b")?,
            '\u{0c}' => write!(w, "\\f")?,
            '\0'..='\u{1f}'
            | '<'
            | '>'
            | '&'
            | '\u{7f}'..='\u{9f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => write!(w, "\\u{:04x}", u32::from(c))?,
            _ => write!(w, "{c}")?,
        }
    }
    write!(w, "\"")?;
    Ok(())
}
fn emit<W: Write>(w: &mut W, n: &Node, depth: usize) -> Result<(), Error> {
    match n {
        Node::Null => write!(w, "null")?,
        Node::Bool(v) => write!(w, "{v}")?,
        Node::Number(v) | Node::Raw(v) => write!(w, "{v}")?,
        Node::Float32(v) => write!(w, "{}", float::f32(*v, float::Style::Json)?)?,
        Node::Float64(v) => write!(w, "{}", float::f64(*v, float::Style::Json)?)?,
        Node::String(v) => string(w, v)?,
        Node::Array(values) => {
            write!(w, "[")?;
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    write!(w, ",")?;
                }
                let next = depth
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid("depth overflow".into()))?;
                write!(w, "\n{}", "  ".repeat(next))?;
                emit(w, v, next)?;
            }
            if !values.is_empty() {
                write!(w, "\n{}", "  ".repeat(depth))?;
            }
            write!(w, "]")?;
        }
        Node::Object(fields) => {
            write!(w, "{{")?;
            for (i, (key, v)) in fields.iter().enumerate() {
                if i > 0 {
                    write!(w, ",")?;
                }
                let next = depth
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid("depth overflow".into()))?;
                write!(w, "\n{}", "  ".repeat(next))?;
                string(w, key)?;
                write!(w, ": ")?;
                emit(w, v, next)?;
            }
            if !fields.is_empty() {
                write!(w, "\n{}", "  ".repeat(depth))?;
            }
            write!(w, "}}")?;
        }
    }
    Ok(())
}
pub struct Projected(Node);

pub fn project<T: ?Sized + Serialize>(value: &T) -> Result<Projected, Error> {
    Ok(Projected(value.serialize(Encoder(0, false))?))
}

impl Projected {
    pub fn for_query(&self) -> Result<Value, Error> {
        value::convert(&self.0)
    }
}

pub fn write_json<W: Write>(w: &mut W, value: &Projected) -> Result<(), Error> {
    emit(w, &value.0, 0)?;
    writeln!(w)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn nested_number_markers_stop_before_recursive_callbacks() {
        struct Nested<'a> {
            remaining: usize,
            calls: &'a Cell<usize>,
        }
        impl Serialize for Nested<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.calls.set(self.calls.get() + 1);
                if self.remaining == 0 {
                    return serializer.serialize_str("1");
                }
                let mut number = serializer.serialize_struct("$serde_json::private::Number", 1)?;
                number.serialize_field(
                    "$serde_json::private::Number",
                    &Nested {
                        remaining: self.remaining - 1,
                        calls: self.calls,
                    },
                )?;
                number.end()
            }
        }
        let calls = Cell::new(0);
        assert!(matches!(
            project(&Nested {
                remaining: 129,
                calls: &calls
            }),
            Err(Error::Invalid(_))
        ));
        assert!(calls.get() <= 2);
    }

    #[test]
    fn ordinary_number_at_maximum_depth() {
        let mut value = serde_json::Value::Number(serde_json::Number::from(123));
        for _ in 0..128 {
            value = serde_json::Value::Array(vec![value]);
        }
        let projected = project(&value).unwrap();
        assert!(write_json(&mut Vec::new(), &projected).is_ok());
        assert!(projected.for_query().is_ok());
    }
}
