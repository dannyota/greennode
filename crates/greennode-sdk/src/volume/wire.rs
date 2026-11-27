use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Number, Value, value::RawValue};

use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, Output},
    response,
    sender::Response,
};
use crate::wire::strict_json::{self, Shape};
use crate::{Error, ErrorClass, Failure, WriteDispatch};

pub(super) const CAP: usize = 8 << 20;
pub(super) const PAGE_SHAPE: &str =
    "page fields are missing, conflicting, or invalid; body withheld";
pub(super) const DETAIL_SHAPE: &str =
    "volume identity or state is missing, conflicting, or invalid; body withheld";
pub(super) const SNAPSHOT_SHAPE: &str = "invalid response shape; body withheld";
const WITHHELD: &str = "volume response withheld";

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(super) struct Document(pub(super) Option<Box<RawValue>>);

impl Output for Document {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(operation: &Operation, reply: &Response) -> Result<Self, Box<Error>> {
        let body = response::classify(
            operation.id,
            reply.status,
            bytes::Bytes::copy_from_slice(&reply.body),
            operation.success,
            (true, operation.sensitive, operation.retry),
            &reply.credentials,
        )?;
        let body = normalize(&body);
        let value = serde_json::from_str(&body).map_err(|_| {
            Box::new(api(
                operation.id,
                None,
                "",
                "response failed to decode; body withheld",
            ))
        })?;
        Ok(Self(Some(value)))
    }
}

fn unicode_escape(bytes: &[u8], offset: usize) -> Option<u16> {
    let escape = bytes.get(offset..offset.checked_add(6)?)?;
    if !escape.starts_with(b"\\u") {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
}

fn normalize(bytes: &[u8]) -> String {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => std::borrow::Cow::Borrowed(text),
        Err(_) => {
            let mut text = String::with_capacity(bytes.len());
            for chunk in bytes.utf8_chunks() {
                text.push_str(chunk.valid());
                text.extend(std::iter::repeat_n('\u{fffd}', chunk.invalid().len()));
            }
            std::borrow::Cow::Owned(text)
        }
    };
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    let mut quoted = false;
    while let Some(&byte) = bytes.get(offset) {
        if byte == b'"' {
            quoted = !quoted;
        } else if quoted && byte == b'\\' {
            if let Some(code) =
                unicode_escape(bytes, offset).filter(|n| (0xd800..=0xdfff).contains(n))
            {
                if code < 0xdc00
                    && unicode_escape(bytes, offset + 6)
                        .is_some_and(|n| (0xdc00..=0xdfff).contains(&n))
                {
                    output.extend_from_slice(&bytes[offset..offset + 12]);
                    offset += 12;
                } else {
                    output.extend_from_slice(b"\\ufffd");
                    offset += 6;
                }
                continue;
            }
            if let Some(&next) = bytes.get(offset + 1) {
                output.extend_from_slice(&[byte, next]);
                offset += 2;
                continue;
            }
        }
        output.push(byte);
        offset += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| text.into_owned())
}

pub(super) type ShapeResult<T> = Result<T, ()>;

pub(super) struct Object<'a>(BTreeMap<String, &'a RawValue>);

impl<'a> Object<'a> {
    pub(super) fn parse(raw: &'a RawValue) -> ShapeResult<Self> {
        if raw.get() == "null" {
            return Ok(Self(BTreeMap::new()));
        }
        serde_json::from_str(raw.get()).map(Self).map_err(|_| ())
    }

    pub(super) fn get(&self, name: &str) -> ShapeResult<Option<&'a RawValue>> {
        self.entry(name)
            .map(|value| value.filter(|value| value.get() != "null"))
    }

    pub(super) fn entry(&self, name: &str) -> ShapeResult<Option<&'a RawValue>> {
        let mut fields = self
            .0
            .iter()
            .filter(|(key, _)| strict_json::field_matches(name, key));
        let value = fields.next().map(|(_, value)| *value);
        if fields.next().is_some() {
            return Err(());
        }
        Ok(value)
    }

    pub(super) fn value<T: FromWire + Default>(&self, name: &str) -> ShapeResult<T> {
        self.optional(name).map(Option::unwrap_or_default)
    }

    pub(super) fn optional<T: FromWire>(&self, name: &str) -> ShapeResult<Option<T>> {
        self.get(name)?.map(T::from_wire).transpose()
    }

    pub(super) fn require<T: FromWire>(&self, name: &str) -> ShapeResult<T> {
        self.optional(name)?.ok_or(())
    }
}

impl Object<'_> {
    pub(super) fn integral(&self, name: &str) -> ShapeResult<i64> {
        self.optional_integral(name).map(Option::unwrap_or_default)
    }

    pub(super) fn optional_integral(&self, name: &str) -> ShapeResult<Option<i64>> {
        self.get(name)?.map(integral).transpose()
    }
}

fn integral(raw: &RawValue) -> ShapeResult<i64> {
    let text = raw.get();
    if !matches!(text.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
        return Err(());
    }
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = whole.bytes().chain(fractional.bytes());
    if digits.clone().all(|digit| digit == b'0') {
        return Ok(0);
    }
    let length = i64::try_from(mantissa.len()).map_err(|_| ())?;
    let exponent = exponent.parse::<i64>().map_err(|_| ())?;
    if !(-length - 20..=length + 20).contains(&exponent) {
        return Err(());
    }
    let scale = exponent - i64::try_from(fractional.len()).map_err(|_| ())?;
    let kept = (whole.len() + fractional.len())
        .saturating_sub(usize::try_from((-scale).max(0)).map_err(|_| ())?);
    if digits.clone().skip(kept).any(|digit| digit != b'0') {
        return Err(());
    }
    let zeros = usize::try_from(scale.max(0)).map_err(|_| ())?;
    if zeros > 19 {
        return Err(());
    }
    let result = digits
        .take(kept)
        .chain(std::iter::repeat_n(b'0', zeros))
        .try_fold(0_i64, |sum, digit| {
            sum.checked_mul(10)?
                .checked_sub(i64::from(digit.checked_sub(b'0')?))
        })
        .ok_or(())?;
    if negative {
        Ok(result)
    } else {
        result.checked_neg().ok_or(())
    }
}

pub(super) trait FromWire: Sized {
    fn from_wire(raw: &RawValue) -> ShapeResult<Self>;
}

pub(super) fn parse<T: DeserializeOwned>(raw: &RawValue) -> ShapeResult<T> {
    serde_json::from_str(if raw.get() == "-0" { "0" } else { raw.get() }).map_err(|_| ())
}

macro_rules! scalar {
    ($($ty:ty),+) => {$(
        impl FromWire for $ty {
            fn from_wire(raw: &RawValue) -> ShapeResult<Self> {
                parse(raw)
            }
        }
    )+};
}
scalar!(String, bool, i64, u64);

impl<T: FromWire + Default> FromWire for Vec<T> {
    fn from_wire(raw: &RawValue) -> ShapeResult<Self> {
        let items: Vec<Option<&RawValue>> = serde_json::from_str(raw.get()).map_err(|_| ())?;
        items
            .into_iter()
            .map(|item| item.map_or_else(|| Ok(T::default()), T::from_wire))
            .collect()
    }
}

impl FromWire for Value {
    fn from_wire(raw: &RawValue) -> ShapeResult<Self> {
        match raw.get().as_bytes().first() {
            Some(b'{') => Object::parse(raw)?
                .0
                .into_iter()
                .map(|(key, value)| Ok((key, Self::from_wire(value)?)))
                .collect::<ShapeResult<_>>()
                .map(Value::Object),
            Some(b'[') => Vec::<Self>::from_wire(raw).map(Value::Array),
            Some(b'0'..=b'9' | b'-') => {
                let amount = raw.get().parse::<f64>().map_err(|_| ())?;
                Number::from_f64(amount).map(Value::Number).ok_or(())
            }
            _ => parse(raw),
        }
    }
}

pub(super) fn decode<T>(
    document: &Document,
    decode: impl FnOnce(&RawValue) -> ShapeResult<T>,
) -> ShapeResult<T> {
    let raw = document.0.as_deref().ok_or(())?;
    strict_json::validate(raw.get().as_bytes(), &[], CAP).map_err(|_| ())?;
    decode(raw)
}

pub(super) trait Redact {
    fn redact(&mut self, evidence: &Evidence);
}

impl Redact for String {
    fn redact(&mut self, evidence: &Evidence) {
        if evidence.text_reflects(self) {
            *self = "<redacted>".into();
        }
    }
}

impl<T: Redact> Redact for Option<T> {
    fn redact(&mut self, evidence: &Evidence) {
        if let Some(value) = self {
            value.redact(evidence);
        }
    }
}

impl<T: Redact> Redact for Vec<T> {
    fn redact(&mut self, evidence: &Evidence) {
        for value in self {
            value.redact(evidence);
        }
    }
}

macro_rules! scalar_redaction {
    ($($ty:ty),+) => {$(
        impl Redact for $ty {
            fn redact(&mut self, _: &Evidence) {}
        }
    )+};
}
scalar_redaction!(bool, i64, u64);

impl Redact for Value {
    fn redact(&mut self, evidence: &Evidence) {
        evidence.redact(self);
    }
}

pub(super) fn screen<T: serde::Serialize>(
    evidence: &Evidence,
    value: T,
    op: &'static str,
) -> Result<T, Box<Error>> {
    if serde_json::to_value(&value).map_or(true, |value| evidence.output_reflects(&value)) {
        let message = if matches!(
            op,
            "volume.ListSnapshotBackends" | "volume.ListSnapshotPolicies"
        ) {
            "snapshot response withheld"
        } else {
            WITHHELD
        };
        return Err(Box::new(api(op, Some(200), "InvalidResponse", message)));
    }
    Ok(value)
}

pub(super) fn shape(op: &'static str, message: &'static str) -> Box<Error> {
    let (status, code) = if message == SNAPSHOT_SHAPE {
        (None, "")
    } else {
        (Some(200), "InvalidResponse")
    };
    Box::new(api(op, status, code, message))
}

pub(super) fn api(op: &'static str, status: Option<u16>, code: &str, message: &str) -> Error {
    crate::quote::request::api(op, status, code, message, false)
}

pub(super) fn invalid(message: impl Into<String>) -> Box<Error> {
    Box::new(Error::new(ErrorClass::InvalidInput).with_detail(message.into()))
}

#[expect(
    clippy::boxed_local,
    reason = "Private request errors are boxed until building a public Failure"
)]
pub(super) fn failure<T>(error: Box<Error>) -> Failure<T> {
    Failure::new(*error, None, WriteDispatch::NotAWrite)
}
