use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Number, Value, value::RawValue};

use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, Output},
    response,
    sender::Response,
};
use crate::wire::strict_json::{self, Shape};
use crate::{ApiError, Error, output_number};

/// Failure while decoding a successful global load balancer response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The body was not valid JSON.
    MalformedJson,
    /// A JSON value did not match the operation's schema.
    InvalidShape,
    /// A retained credential could not be removed without changing its field type.
    ReflectedCredential,
}

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
        let body = response::classify(
            operation.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            operation.success,
            (true, operation.sensitive, operation.retry),
            &response.credentials,
        )?;
        if body.is_empty() {
            return Ok(Self::default());
        }
        let body = go_json(&body);
        let error = |detail| Box::new(response_error(operation, detail));
        let raw = serde_json::from_str::<&RawValue>(&body)
            .map_err(|_| error(DecodeError::MalformedJson))?;
        strict_json::validate_shape(body.as_bytes(), &Self::SCHEMA, operation.cap)
            .map_err(|_| error(DecodeError::InvalidShape))?;
        serde_json::from_str(raw.get()).map_err(|_| error(DecodeError::InvalidShape))
    }
}

pub(super) fn go_json(bytes: &[u8]) -> Cow<'_, str> {
    let text = go_utf8(bytes);
    let bytes = text.as_bytes();
    let mut normalized = String::new();
    let mut start = 0;
    let mut index = 0;
    let mut in_string = false;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'"' {
            in_string = !in_string;
        }
        if !in_string || byte != b'\\' {
            index += 1;
            continue;
        }
        let Some(unit) = utf16_escape(bytes, index) else {
            index += 2;
            continue;
        };
        let end = index + 6;
        if (0xD800..=0xDBFF).contains(&unit)
            && utf16_escape(bytes, end).is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
        {
            index = end + 6;
            continue;
        }
        if (0xD800..=0xDFFF).contains(&unit) {
            if let Some(prefix) = text.get(start..index) {
                normalized.push_str(prefix);
            }
            normalized.push(char::REPLACEMENT_CHARACTER);
            start = end;
        }
        index = end;
    }
    if start == 0 {
        return text;
    }
    if let Some(rest) = text.get(start..) {
        normalized.push_str(rest);
    }
    Cow::Owned(normalized)
}

fn utf16_escape(bytes: &[u8], offset: usize) -> Option<u16> {
    let digits = bytes.get(offset..offset + 6)?.strip_prefix(b"\\u")?;
    let digits = std::str::from_utf8(digits).ok()?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

fn go_utf8(bytes: &[u8]) -> Cow<'_, str> {
    let mut chunks = bytes.utf8_chunks();
    let Some(first) = chunks.next() else {
        return Cow::Borrowed("");
    };
    if first.invalid().is_empty() {
        return Cow::Borrowed(first.valid());
    }
    let mut normalized = String::with_capacity(bytes.len());
    for chunk in std::iter::once(first).chain(chunks) {
        normalized.push_str(chunk.valid());
        normalized.extend(std::iter::repeat_n(
            char::REPLACEMENT_CHARACTER,
            chunk.invalid().len(),
        ));
    }
    Cow::Owned(normalized)
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
