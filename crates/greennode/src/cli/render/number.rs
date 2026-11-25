use serde::Serialize;
use serde::ser::{self, Impossible};

use super::{Error, Node};

pub(super) struct Token;

fn invalid<T>() -> Result<T, Error> {
    Err(Error::Invalid("invalid number token".into()))
}

macro_rules! reject {
    ($name:ident, $ty:ty) => {
        fn $name(self, _: $ty) -> Result<Node, Error> {
            invalid()
        }
    };
}

impl ser::Serializer for Token {
    type Ok = Node;
    type Error = Error;
    type SerializeSeq = Impossible<Node, Error>;
    type SerializeTuple = Impossible<Node, Error>;
    type SerializeTupleStruct = Impossible<Node, Error>;
    type SerializeTupleVariant = Impossible<Node, Error>;
    type SerializeMap = Impossible<Node, Error>;
    type SerializeStruct = Impossible<Node, Error>;
    type SerializeStructVariant = Impossible<Node, Error>;

    reject!(serialize_bool, bool);
    reject!(serialize_i8, i8);
    reject!(serialize_i16, i16);
    reject!(serialize_i32, i32);
    reject!(serialize_i64, i64);
    reject!(serialize_i128, i128);
    reject!(serialize_u8, u8);
    reject!(serialize_u16, u16);
    reject!(serialize_u32, u32);
    reject!(serialize_u64, u64);
    reject!(serialize_u128, u128);
    reject!(serialize_f32, f32);
    reject!(serialize_f64, f64);
    reject!(serialize_char, char);
    reject!(serialize_bytes, &[u8]);

    fn serialize_str(self, value: &str) -> Result<Node, Error> {
        if value.trim() != value || serde_json::from_str::<serde_json::Number>(value).is_err() {
            return invalid();
        }
        Ok(Node::String(value.into()))
    }
    fn serialize_none(self) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, _: &T) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_unit(self) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Node, Error> {
        invalid()
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        invalid()
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Error> {
        invalid()
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        invalid()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        invalid()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Error> {
        invalid()
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct, Error> {
        invalid()
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        invalid()
    }
    fn collect_str<T: ?Sized + std::fmt::Display>(self, _: &T) -> Result<Node, Error> {
        invalid()
    }
}
