use super::model::*;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output},
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{ApiError, Error, ErrorClass};
use http::StatusCode;
use serde::{Deserialize, de::DeserializeOwned};

pub(super) trait Data: DeserializeOwned + Default {
    const SHAPE: Shape<'static>;
    fn valid(&self) -> bool;
    fn reflects(&self, _: &SentCredentials) -> bool {
        false
    }
}

#[derive(Default, Deserialize)]
pub(super) struct Envelope<T> {
    pub(super) data: T,
}

impl<T: Data> Output for Envelope<T> {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("code", Shape::Any),
        field("message", Shape::Any),
        field("data", T::SHAPE),
    ]);
    const CHECK_RENDERED_NUMBERS: bool = true;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if response.status != StatusCode::OK {
            return operation::decode_response(operation, response);
        }
        let invalid = || Box::new(invalid_response(operation));
        let header: serde_json::Value =
            serde_json::from_slice(&response.body).map_err(|_| invalid())?;
        if response.credentials.has_numeric_product_credential()
            || header.get("code").and_then(serde_json::Value::as_i64) != Some(200)
            || header.get("data").is_none_or(serde_json::Value::is_null)
        {
            return Err(invalid());
        }
        let data = &header["data"];
        if operation.id == "ListDatabaseNetworks"
            && data.as_array().is_none_or(|rows| {
                rows.iter()
                    .any(|row| !row.get("subnets").is_some_and(serde_json::Value::is_array))
            })
        {
            return Err(invalid());
        }
        let output: Self =
            operation::decode_response(operation, response).map_err(|_| invalid())?;
        if !output.data.valid() || output.data.reflects(&response.credentials) {
            return Err(invalid());
        }
        Ok(output)
    }
}

pub(super) fn safe_error(operation: &Operation, error: Error) -> Error {
    let Some(api) = error.api_error() else {
        return error;
    };
    if api.operation() != operation.id || api.status() == Some(200) {
        return error;
    }
    let safe = Error::from_api(ApiError::new(
        operation.id,
        api.status(),
        SafeText::new("", &[]),
        SafeText::new("request failed; response withheld", &[]),
        api.retryable(),
    ));
    if error.class() == ErrorClass::Api {
        safe
    } else {
        Error::new(error.class()).with_cause(safe)
    }
}

pub(super) fn invalid_response(operation: &Operation) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation.id,
        Some(200),
        SafeText::new("InvalidResponse", &[]),
        SafeText::new("invalid response; body withheld", &[]),
        false,
    ))
    .with_outer_detail(StatusCode::OK)
}

const fn field(name: &'static str, shape: Shape<'static>) -> ShapeField<'static> {
    ShapeField { name, shape }
}

impl Data for Vec<EngineVersion> {
    const SHAPE: Shape<'static> = Shape::Array(&EngineVersion::SCHEMA);
    fn valid(&self) -> bool {
        self.iter()
            .all(|x| !x.r#type.is_empty() && !x.version.is_empty())
    }
}
impl Data for Vec<Zone> {
    const SHAPE: Shape<'static> = Shape::Array(&Zone::SCHEMA);
    fn valid(&self) -> bool {
        self.iter()
            .all(|x| valid_id(&x.uuid) && !x.status.is_empty())
    }
}
impl Data for Vec<Flavor> {
    const SHAPE: Shape<'static> = Shape::Array(&Flavor::SCHEMA);
    fn valid(&self) -> bool {
        self.iter().all(|x| x.id > 0)
    }
    fn reflects(&self, credentials: &SentCredentials) -> bool {
        self.iter()
            .any(|row| crate::output_number::reflects(row.monthly_cost, credentials, true))
    }
}
impl Data for Vec<Network> {
    const SHAPE: Shape<'static> = Shape::Array(&Network::SCHEMA);
    fn valid(&self) -> bool {
        self.iter().all(|x| {
            valid_id(&x.uuid)
                && x.subnets.iter().all(|s| {
                    valid_id(&s.uuid) && (s.network_uuid.is_empty() || s.network_uuid == x.uuid)
                })
        })
    }
}
impl Data for Instance {
    const SHAPE: Shape<'static> = Instance::SCHEMA;
    fn valid(&self) -> bool {
        valid_id(&self.id) && !self.name.is_empty() && !self.status.is_empty()
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StorageData {
    #[serde(default, deserialize_with = "super::model::null_default")]
    pub(super) project_id: String,
    pub(super) data: Vec<StorageType>,
}
impl Data for StorageData {
    const SHAPE: Shape<'static> = Shape::Object(&[
        field("projectId", Shape::Any),
        field("data", Shape::Array(&StorageType::SCHEMA)),
    ]);
    fn valid(&self) -> bool {
        self.data.iter().all(|x| {
            !x.r#type.is_empty() && x.min_volume_size > 0 && x.max_volume_size >= x.min_volume_size
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InstanceData {
    #[serde(default, deserialize_with = "super::model::null_default")]
    pub(super) project_id: String,
    pub(super) data: Vec<Instance>,
    pub(super) page_object: Page,
}
impl Data for InstanceData {
    const SHAPE: Shape<'static> = Shape::Object(&[
        field("projectId", Shape::Any),
        field("data", Shape::Array(&Instance::SCHEMA)),
        field("pageObject", Page::SCHEMA),
    ]);
    fn valid(&self) -> bool {
        self.data.iter().all(Data::valid)
    }
}

pub(super) fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
