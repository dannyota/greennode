use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::{Number, Value, value::RawValue};

use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, Output},
    response,
    sender::Response,
};
use crate::wire::strict_json::{self, Shape};
use crate::{ApiError, Error};

mod text;

pub(super) const REDACTED: &str = "<redacted>";

pub(super) trait Wire: Default {
    const SHAPE: Shape<'static> = Shape::Any;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error>;
}

pub(super) fn field<'a>(
    fields: &BTreeMap<String, &'a RawValue>,
    name: &str,
) -> Option<&'a RawValue> {
    fields.get(name).copied().or_else(|| {
        fields
            .iter()
            .find_map(|(key, value)| strict_json::field_matches(key, name).then_some(*value))
    })
}

pub(super) fn condition(
    raw: &RawValue,
) -> Result<Option<BTreeMap<String, Value>>, serde_json::Error> {
    let fields = Option::<BTreeMap<String, Value>>::decode(raw)?;
    if fields.as_ref().is_some_and(|fields| {
        fields
            .values()
            .any(|value| !value.is_object() && !value.is_null())
    }) {
        return Err(invalid("invalid condition operator"));
    }
    Ok(fields)
}

impl Wire for String {
    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Option<Self>>(raw.get()).map(Option::unwrap_or_default)
    }
}

impl Wire for bool {
    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Option<Self>>(raw.get()).map(Option::unwrap_or_default)
    }
}

impl Wire for i64 {
    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        match raw.get() {
            "null" | "-0" => Ok(0),
            json => serde_json::from_str(json),
        }
    }
}

impl<T: Wire> Wire for Option<T> {
    const SHAPE: Shape<'static> = T::SHAPE;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        if raw.get() == "null" {
            Ok(None)
        } else {
            T::decode(raw).map(Some)
        }
    }
}

impl<T: Wire> Wire for Vec<T> {
    const SHAPE: Shape<'static> = Shape::Array(&T::SHAPE);

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Option<Vec<&RawValue>>>(raw.get())?
            .unwrap_or_default()
            .into_iter()
            .map(T::decode)
            .collect()
    }
}

impl<T: Wire> Wire for BTreeMap<String, T> {
    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Option<BTreeMap<String, &RawValue>>>(raw.get())?
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| Ok((key, T::decode(value)?)))
            .collect()
    }
}

impl Wire for Value {
    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error> {
        match raw.get().as_bytes().first() {
            Some(b'{') => BTreeMap::<String, Value>::decode(raw)
                .map(|fields| Value::Object(fields.into_iter().collect())),
            Some(b'[') => Vec::decode(raw).map(Value::Array),
            Some(b'-' | b'0'..=b'9') => {
                let number = serde_json::from_str::<f64>(raw.get())?;
                Number::from_f64(number)
                    .map(Value::Number)
                    .ok_or_else(|| invalid("invalid number"))
            }
            _ => serde_json::from_str(raw.get()),
        }
    }
}

pub(super) fn epoch(raw: &RawValue) -> Result<i64, serde_json::Error> {
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    match raw.get().as_bytes().first() {
        Some(b'"') => {
            let value = String::decode(raw)?;
            if value.is_empty() {
                return Ok(0);
            }
            if let Ok(value) = value.parse() {
                return Ok(value);
            }
            let date = OffsetDateTime::parse(&value, &Rfc3339)
                .map_err(|_| invalid("invalid timestamp"))?;
            i64::try_from(date.unix_timestamp_nanos().div_euclid(1_000_000))
                .map_err(|_| invalid("invalid timestamp"))
        }
        Some(b'{') => {
            let fields = serde_json::from_str(raw.get())?;
            strict_json::validate_shape(
                raw.get().as_bytes(),
                &Shape::Object(&[strict_json::ShapeField {
                    name: "$numberLong",
                    shape: Shape::Any,
                }]),
                raw.get().len(),
            )
            .map_err(|_| invalid("invalid timestamp object"))?;
            let value =
                field(&fields, "$numberLong").ok_or_else(|| invalid("missing timestamp"))?;
            String::decode(value)?
                .parse()
                .map_err(|_| invalid("invalid timestamp"))
        }
        _ => i64::decode(raw),
    }
}

pub(super) fn root(raw: &RawValue) -> Result<String, serde_json::Error> {
    match raw.get().as_bytes().first() {
        Some(b'-' | b'0'..=b'9') => Ok(raw.get().to_owned()),
        _ => String::decode(raw),
    }
}

fn invalid(detail: &'static str) -> serde_json::Error {
    serde::de::Error::custom(detail)
}

pub(super) trait Screen {
    fn screen(&mut self, evidence: &Evidence) -> bool;
}

impl Screen for String {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        if evidence.text_reflects(self) {
            *self = REDACTED.to_owned();
        }
        false
    }
}

impl Screen for i64 {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        if evidence.reflected_in(&Value::from(*self)) {
            *self = 0;
            return true;
        }
        false
    }
}

impl Screen for bool {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        if evidence.reflected_in(&Value::Bool(*self)) {
            *self = false;
            return true;
        }
        false
    }
}

impl<T: Screen> Screen for Option<T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        let reflected = match self {
            Some(value) => value.screen(evidence),
            None => evidence.reflected_in(&Value::Null),
        };
        if reflected {
            *self = None;
        }
        reflected
    }
}

impl<T: Screen> Screen for Vec<T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        for value in self {
            value.screen(evidence);
        }
        false
    }
}

impl Screen for Value {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        evidence.redact(self);
        false
    }
}

impl<T: Screen> Screen for BTreeMap<String, T> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.retain(|key, value| !evidence.text_reflects(key) && !value.screen(evidence));
        false
    }
}

#[derive(Default)]
pub(super) struct Body<T>(pub(super) T);

impl<'de, T: Wire> Deserialize<'de> for Body<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::decode(<&RawValue>::deserialize(d)?)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

impl<T: Wire> Output for Body<T> {
    const SCHEMA: Shape<'static> = T::SHAPE;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let label = account_label(operation.id);
        let body = response::classify(
            operation.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            operation.success,
            (true, label.is_some(), operation.retry),
            &response.credentials,
        )?;
        if body.is_empty() && operation.allow_empty {
            return Ok(Self::default());
        }
        let body = text::go_json(&body);
        let error = |detail| Box::new(decode_error(operation, detail));
        let raw = serde_json::from_str::<&RawValue>(&body)
            .map_err(|_| error(super::DecodeError::MalformedJson))?;
        strict_json::validate_shape(body.as_bytes(), &Self::SCHEMA, body.len())
            .map_err(|_| error(super::DecodeError::InvalidShape))?;
        T::decode(raw)
            .map(Self)
            .map_err(|_| error(super::DecodeError::InvalidShape))
    }
}

fn account_label(operation: &str) -> Option<&'static str> {
    match operation {
        "iam.GetServiceAccount" => Some("service account"),
        "iam.ListServiceAccounts" => Some("service accounts"),
        _ => None,
    }
}

pub(super) fn account_error(id: &'static str, error: Error) -> Error {
    let Some((label, api)) = account_label(id).zip(error.api_error()) else {
        return error;
    };
    if api.operation() == id
        && let Some(status) = api.status()
    {
        let message = format!(
            "{id}: read {label}: {id}: IAM user request failed; response withheld (status {status})"
        );
        return Error::from_api(ApiError::without_fallback(
            id,
            Some(status),
            SafeText::new(api.code(), &[]),
            SafeText::new(&message, &[]),
            api.retryable(),
        ));
    }
    if error.is_body_too_large() {
        let message = format!("{id}: read {label}: transport: response body exceeds limit");
        return Error::from_api(ApiError::without_fallback(
            "",
            None,
            SafeText::new("", &[]),
            SafeText::new(&message, &[]),
            false,
        ))
        .with_body_too_large();
    }
    error
}

fn decode_error(operation: &Operation, detail: super::DecodeError) -> Error {
    let id = operation.id;
    let message = if let Some(label) = account_label(id) {
        format!("{id}: read {label}: {id}: response failed to decode; body withheld")
    } else if operation.sensitive {
        "response failed to decode; body withheld".to_owned()
    } else {
        format!(
            "{}: request failed: response failed to decode",
            operation.id
        )
    };
    Error::from_api(ApiError::without_fallback(
        operation.id,
        None,
        SafeText::new("", &[]),
        SafeText::new(&message, &[]),
        false,
    ))
    .with_detail(detail)
}
