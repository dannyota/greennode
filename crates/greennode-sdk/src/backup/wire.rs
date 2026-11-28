use http::StatusCode;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output},
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{ApiError, Error};

use super::{Backend, GET_POLICY, Policy, model::reflects_integer};

const WITHHELD: &str = "Backup Center response withheld";
const INVALID_BODY: &str = "unrecognized Backup Center response; body withheld";

#[derive(Default, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(deserialize = "T: Deserialize<'de> + Default, P: Deserialize<'de>")
)]
pub(super) struct Collection<T, P> {
    #[serde(deserialize_with = "rows")]
    pub(super) items: Vec<T>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub(super) page: P,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub(super) page_size: P,
    pub(super) total_pages: i64,
    pub(super) total_items: i64,
}

fn rows<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let items = Vec::<Option<T>>::deserialize(deserializer)?;
    Ok(items.into_iter().map(Option::unwrap_or_default).collect())
}

pub(super) trait Row: DeserializeOwned + Default {
    const SHAPE: Shape<'static>;

    fn reflects(&self, credentials: &SentCredentials) -> bool;
}

impl Row for Backend {
    const SHAPE: Shape<'static> =
        Shape::Object(&[field("id", Shape::Any), field("name", Shape::Any)]);

    fn reflects(&self, credentials: &SentCredentials) -> bool {
        credentials.id_reflects(&self.id) || credentials.text_reflects(&self.name)
    }
}

impl Row for Policy {
    const SHAPE: Shape<'static> = POLICY_SCHEMA;

    fn reflects(&self, credentials: &SentCredentials) -> bool {
        self.reflects(credentials)
    }
}

impl<T: Row, P: DeserializeOwned + Default + Copy + Into<Option<i64>>> Output for Collection<T, P> {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("items", Shape::Array(&T::SHAPE)),
        field("page", Shape::Any),
        field("pageSize", Shape::Any),
        field("totalPages", Shape::Any),
        field("totalItems", Shape::Any),
    ]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let output: Self = decode(operation, response)?;
        let credentials = &response.credentials;
        if output.items.iter().any(|item| item.reflects(credentials))
            || [output.page.into(), output.page_size.into()]
                .into_iter()
                .flatten()
                .chain([output.total_pages, output.total_items])
                .any(|value| reflects_integer(value, credentials))
        {
            return Err(Box::new(reflected_credential(operation)));
        }
        Ok(output)
    }
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(super) struct Detail {
    pub(super) policy: Policy,
    #[serde(skip)]
    pub(super) reflected: bool,
}

impl Output for Detail {
    const SCHEMA: Shape<'static> = POLICY_SCHEMA;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        #[derive(Deserialize)]
        struct ConfigPresence<'a> {
            #[serde(borrow)]
            config: &'a RawValue,
        }

        let mut output: Self = decode(operation, response)?;
        let invalid = || Box::new(invalid_body(operation));
        let null_config = response.decode_projection(|body| {
            let text = crate::model::wire::go_json(body);
            serde_json::from_str::<ConfigPresence<'_>>(&text)
                .map(|config| config.config.get() == "null")
                .map_err(|_| invalid())
        })?;
        if output.policy.id.is_empty() || null_config {
            return Err(invalid());
        }
        output.reflected = output.policy.reflects(&response.credentials);
        Ok(output)
    }
}

fn decode<T: Output>(operation: &Operation, response: &Response) -> Result<T, Box<Error>> {
    operation::decode_response(operation, response).map_err(|error| {
        if response.status != StatusCode::OK {
            return error;
        }
        if !response.empty_body() && response.malformed_json() {
            return Box::new(response_error(
                operation,
                response.status.as_u16(),
                "",
                "response failed to decode; body withheld",
                false,
            ));
        }
        Box::new(invalid_body(operation))
    })
}

const fn field(name: &'static str, shape: Shape<'static>) -> ShapeField<'static> {
    ShapeField { name, shape }
}

const POLICY_SCHEMA: Shape<'static> = Shape::Object(&[
    field("id", Shape::Any),
    field("backendId", Shape::Any),
    field("projectId", Shape::Any),
    field("product", Shape::Any),
    field("name", Shape::Any),
    field("isDefault", Shape::Any),
    field("backupInstanceCount", Shape::Any),
    field("createdAt", Shape::Any),
    field("updatedAt", Shape::Any),
    field(
        "config",
        Shape::Object(&[
            field("hour", Shape::Any),
            field("minute", Shape::Any),
            field("timeZone", Shape::Any),
            field("hourlyEnabled", Shape::Any),
            field("dailyEnabled", Shape::Any),
            field("weeklyEnabled", Shape::Any),
            field("monthlyEnabled", Shape::Any),
            field("isProtectedServer", Shape::Any),
            field(
                "dailyConfig",
                Shape::Object(&[
                    field("retention", Shape::Any),
                    field("backupType", Shape::Any),
                    field("incrementalQuantity", Shape::Any),
                ]),
            ),
        ]),
    ),
]);

pub(super) fn invalid_body(operation: &Operation) -> Error {
    response_error(operation, StatusCode::OK.as_u16(), "", INVALID_BODY, false)
}

pub(super) fn reflected_credential(operation: &Operation) -> Error {
    response_error(
        operation,
        StatusCode::OK.as_u16(),
        "InvalidResponse",
        WITHHELD,
        false,
    )
}

pub(super) fn withhold(operation: &Operation, error: Error) -> Error {
    if error.is_body_too_large() {
        if operation.id == GET_POLICY
            && error.api_error().and_then(ApiError::observed_status) == Some(200)
        {
            return response_error(operation, 200, "", WITHHELD, false)
                .retain_status_visibility(error.api_error())
                .with_cause(error);
        }
        return Error::bare_body_limit();
    }
    match error.api_error() {
        Some(api) if api.is_for_operation(operation.id) => match api.observed_status() {
            Some(status) if status != 200 => {
                response_error(operation, status, "", WITHHELD, api.retryable())
                    .retain_status_visibility(Some(api))
                    .with_cause(error)
            }
            _ => error,
        },
        _ => error,
    }
}

fn response_error(
    operation: &Operation,
    status: u16,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::new(
        operation.id,
        Some(status),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}
