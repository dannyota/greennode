mod redaction;
mod table;
mod value;
pub use redaction::redact_dynamic;
pub use table::write_table;
pub use value::{Value, normalize, write_text};

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
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

fn exponent(s: String) -> String {
    match s.split_once('e') {
        Some((mantissa, exp)) if !exp.starts_with('-') => format!("{mantissa}e+{exp}"),
        _ => s,
    }
}

const MAX_DEPTH: usize = 128;

struct Encoder(usize);

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
        let n = if v != 0.0 && (v.abs() < 1e-6 || v.abs() >= 1e21) {
            exponent(format!("{v:e}"))
        } else {
            v.to_string()
        };
        Ok(Node::Number(n))
    }
    fn serialize_f64(self, v: f64) -> Result<Node, Error> {
        if !v.is_finite() {
            return Err(Error::Invalid("nonfinite float".into()));
        }
        let n = if v != 0.0 && (v.abs() < 1e-6 || v.abs() >= 1e21) {
            exponent(format!("{v:e}"))
        } else {
            v.to_string()
        };
        Ok(Node::Number(n))
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
        _: &'static str,
        v: &T,
    ) -> Result<Node, Error> {
        v.serialize(Encoder(self.descend()?))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Node, Error> {
        Ok(Node::Object(vec![(
            variant.into(),
            value.serialize(Encoder(self.descend()?))?,
        )]))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?))
    }
    fn serialize_tuple(self, _: usize) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?))
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Seq, Error> {
        Ok(Seq(Vec::new(), self.descend()?))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<VariantSeq, Error> {
        Ok(VariantSeq(variant.into(), Vec::new(), self.descend()?))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Map, Error> {
        Ok(Map {
            fields: Vec::new(),
            key: None,
            sort: true,
            number: false,
            depth: self.descend()?,
        })
    }
    fn serialize_struct(self, name: &'static str, _: usize) -> Result<Map, Error> {
        Ok(Map {
            fields: Vec::new(),
            key: None,
            sort: false,
            number: name == "$serde_json::private::Number",
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
        Ok(VariantMap(variant.into(), Vec::new(), self.descend()?))
    }
}

struct Seq(Vec<Node>, usize);
impl SerializeSeq for Seq {
    type Ok = Node;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        self.0.push(v.serialize(Encoder(self.1))?);
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
struct VariantSeq(String, Vec<Node>, usize);
impl SerializeTupleVariant for VariantSeq {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        self.1.push(v.serialize(Encoder(self.2))?);
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        Ok(Node::Object(vec![(self.0, Node::Array(self.1))]))
    }
}
struct Map {
    fields: Vec<(String, Node)>,
    key: Option<String>,
    sort: bool,
    number: bool,
    depth: usize,
}
impl SerializeMap for Map {
    type Ok = Node;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        if self.key.is_some() {
            return Err(Error::Invalid("missing map value".into()));
        }
        self.key = Some(match key.serialize(Encoder(self.depth))? {
            Node::String(s) | Node::Number(s) => s,
            Node::Bool(b) => b.to_string(),
            _ => return Err(Error::Invalid("invalid map key".into())),
        });
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error::Invalid("missing map key".into()))?;
        self.fields.push((key, v.serialize(Encoder(self.depth))?));
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
        self.fields
            .push((key.into(), v.serialize(Encoder(self.depth))?));
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        SerializeMap::end(self)
    }
}
struct VariantMap(String, Vec<(String, Node)>, usize);
impl SerializeStructVariant for VariantMap {
    type Ok = Node;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        self.1.push((key.into(), v.serialize(Encoder(self.2))?));
        Ok(())
    }
    fn end(self) -> Result<Node, Error> {
        Ok(Node::Object(vec![(self.0, Node::Object(self.1))]))
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
        Node::Number(v) => write!(w, "{v}")?,
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
pub fn write_json<W: Write, T: ?Sized + Serialize>(w: &mut W, value: &T) -> Result<(), Error> {
    let n = value.serialize(Encoder(0))?;
    emit(w, &n, 0)?;
    writeln!(w)?;
    Ok(())
}
