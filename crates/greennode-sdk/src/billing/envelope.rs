use super::model::{Budget, field};
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output},
    response,
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::{self, Shape, ShapeField};
use crate::{ApiError, Error, ErrorClass};
use http::StatusCode;
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::borrow::Cow;

#[derive(Default, Deserialize)]
pub(super) struct Envelope<T> {
    pub(super) data: T,
}

pub(super) trait BudgetData: DeserializeOwned + Default {
    const SCHEMA: Shape<'static>;

    fn valid(&self, credentials: &SentCredentials) -> bool;

    fn redact(&mut self, _: &SentCredentials) {}
}

impl BudgetData for Budget {
    const SCHEMA: Shape<'static> = Budget::SCHEMA;

    fn valid(&self, credentials: &SentCredentials) -> bool {
        !self.uuid.trim().is_empty() && !self.reflects_numbers(credentials)
    }
}

impl BudgetData for Vec<Budget> {
    const SCHEMA: Shape<'static> = Shape::Array(&Budget::SCHEMA);

    fn valid(&self, credentials: &SentCredentials) -> bool {
        self.iter()
            .all(|budget| !budget.reflects_numbers(credentials))
    }
}

impl BudgetData for () {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn valid(&self, _: &SentCredentials) -> bool {
        true
    }
}

const HEADER_SCHEMA: Shape<'static> =
    Shape::Object(&[field("code"), field("message"), field("data")]);

impl<T: BudgetData> Output for Envelope<T> {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("code"),
        field("message"),
        ShapeField {
            name: "data",
            shape: T::SCHEMA,
        },
    ]);

    fn reflected_response(operation: &Operation, response: &Response) -> Box<Error> {
        if let Ok(header) = serde_json::from_slice::<Header<'_>>(&response.body)
            && header
                .screened_code(&response.credentials)
                .is_some_and(|code| success_code(code) == Some(false))
        {
            return api_error(operation, response, header.not_found(&response.credentials));
        }
        Box::new(invalid_response(operation, response.status, false))
    }

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let status = response.status;
        let invalid = || Box::new(invalid_response(operation, status, false));
        if status == StatusCode::BAD_REQUEST
            && serde_json::from_slice::<Header<'_>>(&response.body).is_ok_and(|header| {
                (header.code.is_none() || header.screened_code(&response.credentials).is_some())
                    && header.not_found(&response.credentials)
            })
        {
            return Err(api_error(operation, response, true));
        }
        if !operation.success.contains(&status) {
            if status.is_success() {
                return Err(invalid());
            }
            return operation::decode_response(operation, response);
        }
        if operation.allow_empty && response.body.is_empty() {
            return Ok(Self::default());
        }
        let header: Header<'_> = serde_json::from_slice(&response.body).map_err(|_| invalid())?;
        let code = header
            .screened_code(&response.credentials)
            .ok_or_else(invalid)?;
        if !success_code(code).ok_or_else(invalid)? {
            strict_json::validate_shape(&response.body, &HEADER_SCHEMA, operation.cap)
                .map_err(|_| invalid())?;
            return Err(api_error(
                operation,
                response,
                header.not_found(&response.credentials),
            ));
        }
        if response.credentials.has_numeric_product_credential() {
            return Err(invalid());
        }
        if operation.no_output {
            strict_json::validate_shape(&response.body, &Self::SCHEMA, operation.cap)
                .map_err(|_| invalid())?;
        }
        let mut output: Self =
            operation::decode_response(operation, response).map_err(|_| invalid())?;
        if !output.data.valid(&response.credentials) {
            return Err(invalid());
        }
        output.data.redact(&response.credentials);
        Ok(output)
    }
}

#[derive(Deserialize)]
struct Header<'a> {
    #[serde(borrow, default, deserialize_with = "present_code")]
    code: Option<&'a RawValue>,
    #[serde(borrow)]
    message: Option<Cow<'a, str>>,
}

impl Header<'_> {
    fn screened_code(&self, credentials: &SentCredentials) -> Option<&str> {
        let wire = self.code?.get();
        if credentials.text_reflects(wire) {
            return None;
        }
        if wire.starts_with('"') {
            let decoded = serde_json::from_str::<String>(wire).ok()?;
            response::decision_text(&decoded, credentials)?;
        }
        Some(wire)
    }

    fn not_found(&self, credentials: &SentCredentials) -> bool {
        self.message.as_deref().is_some_and(|message| {
            let safe = response::sanitize_error_text(message, credentials);
            let message = safe.as_str().trim_start();
            message.starts_with("Budget not found") || message.starts_with("Threshold not found")
        })
    }
}

fn present_code<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

fn success_code(code: &str) -> Option<bool> {
    match code.as_bytes().first()? {
        b'n' if code == "null" => Some(true),
        b'0'..=b'9' | b'-' => Some(matches!(code.parse::<u16>(), Ok(200..=299))),
        b'"' => {
            let text: String = serde_json::from_str(code).ok()?;
            (!text.is_empty() && !matches!(text.parse::<i64>(), Ok(200..=299))).then_some(false)
        }
        _ => None,
    }
}

fn api_error(operation: &Operation, reply: &Response, not_found: bool) -> Box<Error> {
    let error = response::classify(
        operation.id,
        reply.status,
        bytes::Bytes::copy_from_slice(&reply.body),
        &[],
        (true, operation.sensitive, operation.retry),
        &reply.credentials,
    )
    .err()
    .unwrap_or_else(|| Box::new(invalid_response(operation, reply.status, false)));
    if not_found && let Some(api) = error.api_error() {
        let secrets: Vec<_> = reply.credentials.values().iter().collect();
        let detail = ApiError::without_fallback(
            api.operation(),
            api.status(),
            SafeText::new("NotFound", &secrets),
            SafeText::new(api.message(), &secrets),
            api.retryable(),
        );
        Box::new(Error::new(ErrorClass::NotFound).with_cause(Error::from_api(detail)))
    } else {
        error
    }
}

pub(super) fn invalid_response(operation: &Operation, status: StatusCode, overflow: bool) -> Error {
    let message = if overflow {
        "billing response exceeds size limit; body withheld"
    } else {
        "invalid billing response; body withheld"
    };
    Error::from_api(ApiError::without_fallback(
        operation.id,
        Some(status.as_u16()),
        SafeText::new("InvalidResponse", &[]),
        SafeText::new(message, &[]),
        false,
    ))
    .with_outer_detail(status)
}
