pub(super) use crate::model::wire::go_json;
use std::collections::BTreeMap;

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Number, Value, value::RawValue};

use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, Output},
    sender::Response,
};
use crate::wire::strict_json::{self, Shape};
use crate::{ApiError, Error, output_number};

/// Failure while decoding a successful global load balancer response.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The body was not valid JSON.
    MalformedJson,
    /// A JSON value did not match the operation's schema.
    InvalidShape,
    /// A retained credential could not be removed without changing its field type.
    ReflectedCredential,
}

crate::error::debug_record!(DecodeError; MalformedJson, InvalidShape, ReflectedCredential);

pub(super) trait WireField: Default + DeserializeOwned {
    const SHAPE: Shape<'static> = Shape::Any;

    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<Self>::deserialize(d).map(Option::unwrap_or_default)
    }

    fn redact(&mut self, evidence: &Evidence) -> bool;

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool
    where
        Self: Serialize,
    {
        serde_json::to_value(&*self).is_ok_and(|value| !evidence.identifier_output_reflects(&value))
            && self.redact(evidence)
    }
}

pub(super) fn deserialize_field<'de, D: Deserializer<'de>, T: WireField>(
    d: D,
) -> Result<T, D::Error> {
    T::read(d)
}

pub(super) struct Field<T>(pub(super) T);

impl<'de, T: WireField> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::read(d).map(Self)
    }
}

impl<T: WireField> WireField for Vec<T> {
    const SHAPE: Shape<'static> = Shape::Array(&T::SHAPE);

    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let rows = Option::<Vec<Field<T>>>::deserialize(d)?.unwrap_or_default();
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.iter_mut().all(|item| item.redact(evidence))
    }
}

impl<T: WireField> WireField for Option<T> {
    const SHAPE: Shape<'static> = T::SHAPE;

    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::deserialize(d)
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.as_mut().is_none_or(|item| item.redact(evidence))
    }
}

impl WireField for String {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        if evidence.text_reflects(self) {
            *self = "<redacted>".into();
            return !evidence.text_reflects(self);
        }
        true
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        if evidence.id_reflects(self) {
            *self = "<redacted>".into();
        }
        !evidence.id_reflects(self)
    }
}

impl WireField for i64 {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match <&RawValue>::deserialize(d)?.get() {
            "null" | "-0" => Ok(0),
            value => serde_json::from_str(value).map_err(serde::de::Error::custom),
        }
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence.integer_output_reflects(*self)
    }
}

impl WireField for f64 {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        !output_number::reflects(*self, evidence.credentials(), true)
    }
}

impl WireField for bool {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence
            .credentials()
            .reflects(if *self { b"true" } else { b"false" })
    }
}

impl WireField for Value {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(d)?;
        dynamic(raw).map_err(serde::de::Error::custom)
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        evidence.redact(self)
    }
}

fn dynamic(raw: &RawValue) -> Result<Value, serde_json::Error> {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => serde_json::from_str::<BTreeMap<String, &RawValue>>(text)?
            .into_iter()
            .map(|(key, value)| Ok((key, dynamic(value)?)))
            .collect::<Result<_, _>>()
            .map(Value::Object),
        Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(text)?
            .into_iter()
            .map(dynamic)
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Some(b'-' | b'0'..=b'9') => {
            let value = serde_json::from_str::<f64>(text)?;
            Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| serde::de::Error::custom("invalid number"))
        }
        _ => serde_json::from_str(text),
    }
}

pub(super) fn serialize_detail<S: Serializer>(value: &Value, s: S) -> Result<S::Ok, S::Error> {
    Detail {
        value,
        marked: false,
    }
    .serialize(s)
}

struct Detail<'a> {
    value: &'a Value,
    marked: bool,
}

impl Serialize for Detail<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Object(_) if !self.marked => s.serialize_newtype_struct(
                "$greennode::dynamic-map",
                &Detail {
                    value: self.value,
                    marked: true,
                },
            ),
            Value::Object(object) => {
                let mut map = s.serialize_map(Some(object.len()))?;
                for (key, value) in object {
                    map.serialize_entry(
                        key,
                        &Detail {
                            value,
                            marked: true,
                        },
                    )?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut list = s.serialize_seq(Some(values.len()))?;
                for value in values {
                    list.serialize_element(&Detail {
                        value,
                        marked: self.marked,
                    })?;
                }
                list.end()
            }
            Value::Number(number) => number
                .as_f64()
                .ok_or_else(|| serde::ser::Error::custom("invalid number"))?
                .serialize(s),
            value => value.serialize(s),
        }
    }
}

impl<T: WireField> Default for Field<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T: WireField> Output for Field<T> {
    const SCHEMA: Shape<'static> = T::SHAPE;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        response.classify(
            operation.id,
            operation.success,
            (true, operation.sensitive, operation.retry),
            false,
        )?;
        response.decode_projection(|body| {
            if body.is_empty() {
                return Ok(Self::default());
            }
            let error = |detail| Box::new(response_error(operation, detail));
            if body.len() > operation.cap {
                return Err(error(DecodeError::InvalidShape));
            }
            let body = go_json(body);
            let raw = serde_json::from_str::<&RawValue>(&body)
                .map_err(|_| error(DecodeError::MalformedJson))?;
            strict_json::validate_shape(body.as_bytes(), &Self::SCHEMA, body.len())
                .map_err(|_| error(DecodeError::InvalidShape))?;
            serde_json::from_str(raw.get()).map_err(|_| error(DecodeError::InvalidShape))
        })
    }
}

pub(super) fn response_error(operation: &Operation, detail: DecodeError) -> Error {
    let (code, message) = match detail {
        DecodeError::MalformedJson | DecodeError::InvalidShape => (
            "",
            format!(
                "{}: request failed: response failed to decode",
                operation.id
            ),
        ),
        DecodeError::ReflectedCredential => (
            "InvalidResponse",
            "global load balancer response withheld".into(),
        ),
    };
    Error::from_api(ApiError::without_fallback(
        operation.id,
        None,
        SafeText::new(code, &[]),
        SafeText::new(&message, &[]),
        false,
    ))
    .with_detail(detail)
}
