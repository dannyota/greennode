use std::collections::BTreeMap;

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
mod timestamp;

pub(super) const REDACTED: &str = "<redacted>";

pub(super) trait Wire: Default {
    const SHAPE: Shape<'static> = Shape::Any;

    fn decode(raw: &RawValue) -> Result<Self, serde_json::Error>;
}

pub(super) fn field<'a>(
    fields: &BTreeMap<String, &'a RawValue>,
    name: &str,
) -> Option<&'a RawValue> {
    fields.get(name).copied()
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

pub(super) fn condition_output(fields: &Option<BTreeMap<String, Value>>) -> impl Serialize + '_ {
    fields.as_ref().map(|fields| ConditionMap(fields.iter()))
}

struct ConditionMap<I>(I);

impl<'a, I: Clone + ExactSizeIterator<Item = (&'a String, &'a Value)>> Serialize
    for ConditionMap<I>
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0.clone() {
            map.serialize_entry(key, &ConditionValue(value))?;
        }
        map.end()
    }
}

struct ConditionValue<'a>(&'a Value);

impl Serialize for ConditionValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(fields) => ConditionMap(fields.iter()).serialize(serializer),
            Value::Array(items) => {
                let mut list = serializer.serialize_seq(Some(items.len()))?;
                for value in items {
                    list.serialize_element(&ConditionValue(value))?;
                }
                list.end()
            }
            Value::Number(number) => number
                .as_f64()
                .ok_or_else(|| serde::ser::Error::custom("invalid condition number"))?
                .serialize(serializer),
            other => other.serialize(serializer),
        }
    }
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

pub(super) fn exact_value(raw: &RawValue) -> Result<Value, serde_json::Error> {
    match raw.get().as_bytes().first() {
        Some(b'{') => serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get())?
            .into_iter()
            .map(|(key, value)| Ok((key, exact_value(value)?)))
            .collect::<Result<_, _>>()
            .map(Value::Object),
        Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(raw.get())?
            .into_iter()
            .map(exact_value)
            .collect::<Result<_, _>>()
            .map(Value::Array),
        _ => serde_json::from_str(raw.get()),
    }
}

pub(super) fn epoch(raw: &RawValue) -> Result<i64, serde_json::Error> {
    match raw.get().as_bytes().first() {
        Some(b'"') => {
            let value = String::decode(raw)?;
            if value.is_empty() {
                return Ok(0);
            }
            if let Ok(value) = value.parse() {
                return Ok(value);
            }
            timestamp::parse(&value).ok_or_else(|| invalid("invalid timestamp"))
        }
        Some(b'{') => {
            let fields = serde_json::from_str(raw.get())?;
            strict_json::validate_exact_shape(
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

pub(super) trait ScreenId {
    fn screen_id(&mut self, evidence: &Evidence) -> bool;
}

impl ScreenId for String {
    fn screen_id(&mut self, evidence: &Evidence) -> bool {
        if evidence.id_reflects(self) {
            *self = REDACTED.to_owned();
        }
        false
    }
}

impl ScreenId for i64 {
    fn screen_id(&mut self, evidence: &Evidence) -> bool {
        if evidence.id_reflects(&self.to_string()) {
            *self = 0;
            return true;
        }
        self.screen(evidence)
    }
}

impl ScreenId for Vec<String> {
    fn screen_id(&mut self, evidence: &Evidence) -> bool {
        for id in self {
            id.screen_id(evidence);
        }
        false
    }
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
        let account = account_label(operation.id).is_some();
        let private = operation.sensitive && operation.id != "iam.GetUser";
        let body = response::classify(
            operation.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            operation.success,
            (true, account || private, operation.retry),
            &response.credentials,
        )?;
        if operation.no_output || (body.is_empty() && operation.allow_empty) {
            return Ok(Self::default());
        }
        let body = text::go_json(&body);
        let error = |detail| Box::new(decode_error(operation, detail));
        let raw = serde_json::from_str::<&RawValue>(&body)
            .map_err(|_| error(super::DecodeError::MalformedJson))?;
        strict_json::validate_exact_shape(body.as_bytes(), &Self::SCHEMA, body.len())
            .map_err(|_| error(super::DecodeError::InvalidShape))?;
        let value = T::decode(raw).map_err(|_| error(super::DecodeError::InvalidShape))?;
        if operation.reject_reflected_credentials
            && super::operation::reflects_json_forms(body.as_bytes(), &response.credentials)
                != Some(false)
        {
            return Err(Box::new(Error::new(crate::ErrorClass::Api).with_detail(
                crate::RecoveryAdvice::new(if operation.id == "iam.GetServiceAccount" {
                    "iam: iam.GetServiceAccount: unsafe guard response"
                } else {
                    "IAM user response withheld"
                }),
            )));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ObservedResponse {
    ClientRejection,
    NoContent,
    Other,
}

impl ObservedResponse {
    pub(super) fn from_status(status: http::StatusCode) -> Self {
        if status.is_client_error() {
            Self::ClientRejection
        } else if status == http::StatusCode::NO_CONTENT {
            Self::NoContent
        } else {
            Self::Other
        }
    }

    pub(super) fn status(error: &Error) -> Option<http::StatusCode> {
        error.detail::<http::StatusCode>().copied().or_else(|| {
            error
                .api_error()
                .and_then(|api| api.status())
                .and_then(|status| http::StatusCode::from_u16(status).ok())
        })
    }
}

pub(super) fn private_status_error(
    operation: &Operation,
    error: Error,
    evidence: &Evidence,
) -> Error {
    if !operation.sensitive
        || operation.id == "iam.GetUser"
        || account_label(operation.id).is_some()
    {
        return error;
    }
    let Some(api) = error
        .api_error()
        .filter(|api| api.operation() == operation.id)
    else {
        return error;
    };
    let Some(status) =
        ObservedResponse::status(&error).filter(|status| !operation.success.contains(status))
    else {
        return error;
    };
    const MESSAGE: &str = "IAM user request failed; response withheld";
    let label = match operation.template {
        "accounts-api/v1/auth/userinfo" => Some("caller identity"),
        "policies-api/v1/actions" => Some("action list"),
        "policies-api/v1/policies/{}" => Some("policy"),
        "policies-api/v1/user-attachments/service-accounts/{}/policies"
        | "policies-api/v1/user-attachments/iam-users/{}/policies" => Some("policy attachments"),
        "policies-api/v1/user-attachments/iam-users/{}/groups" => Some("attachment array"),
        _ => None,
    };
    let message = label.map_or_else(
        || MESSAGE.to_owned(),
        |label| {
            let id = operation.id;
            format!(
                "{id}: read {label}: {id}: {MESSAGE} (status {})",
                status.as_u16()
            )
        },
    );
    let private = Error::from_api(ApiError::new(
        operation.id,
        Some(status.as_u16()),
        SafeText::new("", &[]),
        SafeText::new(&message, &[]),
        api.retryable(),
    ));
    private
        .with_cause(error)
        .screen_api(&|value| response::reflects_credential_forms(value, evidence.credentials()))
        .with_outer_detail(ObservedResponse::from_status(status))
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
