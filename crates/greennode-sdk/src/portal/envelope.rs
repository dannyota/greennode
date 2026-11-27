use super::UserInfo;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output},
    sender::Response,
};
use crate::wire::strict_json::{Shape, field_matches};
use crate::{ApiError, Error, ErrorClass};
use http::StatusCode;
use serde::{Deserialize, Deserializer, de};
use serde_json::{Number, Value, value::RawValue};
use std::collections::BTreeMap;

const ENVELOPES: &[&str] = &["data", "user", "userInfo", "quota"];

#[derive(Default)]
pub(super) struct Envelope(pub(super) UserInfo);

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = BTreeMap::<String, &RawValue>::deserialize(deserializer)?;
        let mut envelope = None;
        for key in fields.keys() {
            if let Some(name) = ENVELOPES
                .iter()
                .copied()
                .find(|name| field_matches(key, name))
                && (key != name || envelope.replace(name).is_some())
            {
                return Err(de::Error::custom("invalid portal envelope"));
            }
        }
        if let Some(raw) = envelope.and_then(|name| fields.remove(name)) {
            fields = serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
        }
        let fields = decode_fields(fields).map_err(de::Error::custom)?;
        Ok(Self(UserInfo { fields }))
    }
}

pub(super) fn decode_fields(
    fields: BTreeMap<String, &RawValue>,
) -> serde_json::Result<BTreeMap<String, Value>> {
    fields
        .into_iter()
        .map(|(name, raw)| decode_value(raw).map(|value| (name, value)))
        .collect()
}

fn decode_value(raw: &RawValue) -> serde_json::Result<Value> {
    match raw.get().as_bytes().first() {
        Some(b'{') => {
            let fields = serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get())?;
            Ok(Value::Object(decode_fields(fields)?.into_iter().collect()))
        }
        Some(b'[') => {
            let values = serde_json::from_str::<Vec<&RawValue>>(raw.get())?;
            values
                .into_iter()
                .map(decode_value)
                .collect::<Result<_, _>>()
                .map(Value::Array)
        }
        Some(b'-' | b'0'..=b'9') => Ok(Value::Number(Number::from_string_unchecked(
            raw.get().to_owned(),
        ))),
        _ => serde_json::from_str(raw.get()),
    }
}

impl Output for Envelope {
    const SCHEMA: Shape<'static> = Shape::Object(&[]);

    fn reflected_response(operation: &Operation, response: &Response) -> Box<Error> {
        Box::new(invalid_response(operation, response))
    }

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if response.status != StatusCode::OK
            || (!response.body.is_empty()
                && serde_json::from_slice::<&RawValue>(&response.body).is_err())
        {
            return operation::decode_response(operation, response);
        }
        let output: Self = operation::decode_response(operation, response)
            .map_err(|_| Box::new(invalid_response(operation, response)))?;
        let evidence =
            crate::wire::product::evidence::Evidence::from_credentials(&response.credentials);
        if evidence.serialized_output_reflects(&output.0.fields) {
            return Err(Box::new(invalid_response(operation, response)));
        }
        Ok(output)
    }
}

pub(super) fn invalid_response(operation: &Operation, response: &Response) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation.id,
        Some(response.status.as_u16()),
        SafeText::new("InvalidResponse", &[]),
        SafeText::new("invalid portal response; body withheld", &[]),
        false,
    ))
}

pub(super) fn withheld(operation: &Operation, error: Error) -> Error {
    let api = error.api_error();
    if error.has_class(ErrorClass::InvalidConfig)
        || api.is_some_and(|api| api.code() == "InvalidResponse")
    {
        return error;
    }
    let status = if api.is_none() && error.has_class(ErrorClass::Auth) {
        None
    } else {
        error
            .http_status()
            .map(|status| status.as_u16())
            .or_else(|| api.and_then(ApiError::status))
    };
    let retryable = api.is_some_and(ApiError::retryable);
    Error::from_api(ApiError::new(
        operation.id,
        status,
        SafeText::new("", &[]),
        SafeText::new("portal response withheld", &[]),
        retryable,
    ))
    .with_cause(error)
}
