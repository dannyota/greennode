use super::model::{Budget, field};
use super::response_validation::data_valid;
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
        if matches!(
            operation.class,
            crate::wire::product::redirect::Classification::Read
        ) {
            let error = api_error(operation, response);
            if error.api_error().is_some_and(|api| {
                !api.code().is_empty()
                    && !api
                        .code()
                        .parse::<u16>()
                        .is_ok_and(|code| (200..300).contains(&code))
            }) {
                return error;
            }
        }
        Box::new(invalid_response(operation, response.status, false))
    }

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let status = response.status;
        let invalid = || Box::new(invalid_response(operation, status, false));
        if !operation.success.contains(&status) {
            if status.is_success() {
                return Err(invalid());
            }
            return Err(api_error(operation, response));
        }
        response
            .decode_projection(|body| {
                if operation.allow_empty && body.is_empty() {
                    return Ok(Self::default());
                }
                let header = decode_header(body, operation.cap).ok_or_else(invalid)?;
                let (code, _) = header
                    .proved_code(&response.credentials)
                    .ok_or_else(invalid)?;
                Self::decoded_projection(operation, response, &header, code, body)
            })
            .map_err(|error| {
                if error.detail::<operation::RefusedGuard>().is_some() {
                    if error.has_identity(crate::ErrorClass::NotSettled) {
                        Box::new(
                            crate::Error::new(crate::ErrorClass::NotSettled)
                                .with_cause(*invalid())
                                .with_cause(*error),
                        )
                    } else {
                        invalid()
                    }
                } else {
                    error
                }
            })
    }
}

impl<T: BudgetData> Envelope<T> {
    fn decoded_projection(
        operation: &Operation,
        response: &Response,
        header: &Header<'_>,
        code: &str,
        body: &[u8],
    ) -> Result<Self, Box<Error>> {
        let status = response.status;
        let invalid = || Box::new(invalid_response(operation, status, false));
        if !success_code(code).ok_or_else(invalid)? {
            return Err(api_error(operation, response));
        }
        if response.credentials.has_numeric_product_credential()
            || (!operation.no_output && !data_valid(operation.id, header.data))
        {
            return Err(invalid());
        }
        strict_json::validate_canonical_shape(body, &Self::SCHEMA, operation.cap)
            .map_err(|_| invalid())?;
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
pub(super) struct Header<'a> {
    #[serde(borrow, default, deserialize_with = "present_code")]
    code: Option<&'a RawValue>,
    #[serde(borrow, rename = "message")]
    _message: Option<Cow<'a, str>>,
    #[serde(borrow, default, deserialize_with = "present_code")]
    data: Option<&'a RawValue>,
}

pub(super) fn decode_header(body: &[u8], cap: usize) -> Option<Header<'_>> {
    strict_json::validate_folded_canonical_shape(body, &HEADER_SCHEMA, cap).ok()?;
    serde_json::from_slice(body).ok()
}

impl Header<'_> {
    pub(super) fn is_direct(&self) -> bool {
        self.code.is_none() && self.data.is_none()
    }

    fn proved_code(&self, credentials: &SentCredentials) -> Option<(&str, response::Clean)> {
        let raw = self.code?.get();
        let clean = crate::wire::product::evidence::Evidence::from_credentials(credentials)
            .peer_body_screen(raw.as_bytes())
            .clean()?;
        Some((self.screened_code(credentials)?, clean))
    }

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

fn api_error(operation: &Operation, reply: &Response) -> Box<Error> {
    let rules = (true, operation.sensitive, operation.retry);
    let error = if reply.status.is_success() {
        reply.billing_envelope_refusal(operation.id)
    } else {
        reply.classify_billing_transport(operation.id, rules).err()
    }
    .unwrap_or_else(|| Box::new(invalid_response(operation, reply.status, false)));
    let Some(api) = error.api_error() else {
        return error;
    };
    let message = api.message().trim();
    if (reply.status == StatusCode::BAD_REQUEST || reply.status.is_success())
        && reply.clean_receipt().is_some()
        && (message.starts_with("Budget not found") || message.starts_with("Threshold not found"))
    {
        let secrets: Vec<_> = reply.credentials.values().iter().collect();
        let detail = ApiError::without_fallback(
            operation.id,
            api.observed_status(),
            SafeText::new("NotFound", &secrets),
            SafeText::new(api.message(), &secrets),
            api.retryable(),
        );
        Box::new(
            Error::new(ErrorClass::NotFound)
                .with_cause(Error::from_api(detail))
                .retain_status_visibility(Some(api)),
        )
    } else {
        error
    }
}

pub(super) fn read_failure_response(operation: &Operation, error: Error) -> Error {
    let Some(api) = error.api_error() else {
        return error;
    };
    let Some(status) = api
        .observed_status()
        .and_then(|status| StatusCode::from_u16(status).ok())
    else {
        return error;
    };
    if status.is_success()
        && api.code() != "InvalidResponse"
        && (error.is_body_too_large()
            || error.detail::<operation::BodyReadFailure>().is_some()
            || error.detail::<operation::DecodeFailure>().is_some())
    {
        invalid_response(operation, status, error.is_body_too_large()).with_cause(error)
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
    .with_http_status(status)
}
