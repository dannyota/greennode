use super::model::*;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output, RefusedGuard},
    sender::{Response, SentCredentials},
};
use crate::wire::strict_json::{Shape, ShapeField, field_matches};
use crate::{ApiError, Error, ErrorClass};
use http::StatusCode;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

pub(super) trait Data: DeserializeOwned + Default {
    const SHAPE: Shape<'static>;
    fn valid(&self) -> bool;
    fn canonical_fields(_: &RawValue) -> bool {
        true
    }
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

    fn reflected_response(operation: &Operation, _: &Response) -> Box<Error> {
        Box::new(invalid_response(operation))
    }

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if response.status != StatusCode::OK {
            return operation::decode_response(operation, response);
        }
        let invalid = || Box::new(invalid_response(operation));
        #[derive(Deserialize)]
        struct Header<'a> {
            code: i64,
            #[serde(borrow)]
            data: &'a RawValue,
        }
        response.decode_projection(|body| {
            let text = crate::model::wire::go_json(body);
            let header: Header<'_> = serde_json::from_str(&text).map_err(|_| invalid())?;
            if response.credentials.has_numeric_product_credential()
                || header.code != 200
                || header.data.get() == "null"
                || !T::canonical_fields(header.data)
            {
                return Err(invalid());
            }
            if operation.id == "ListDatabaseNetworks" {
                #[derive(Deserialize)]
                struct NetworkRow<'a> {
                    #[serde(borrow)]
                    subnets: &'a RawValue,
                }
                let rows: Vec<NetworkRow<'_>> =
                    serde_json::from_str(header.data.get()).map_err(|_| invalid())?;
                if rows.iter().any(|row| !row.subnets.get().starts_with('[')) {
                    return Err(invalid());
                }
            }
            let output: Self =
                operation::decode_response(operation, response).map_err(|_| invalid())?;
            if !output.data.valid() || output.data.reflects(&response.credentials) {
                return Err(invalid());
            }
            Ok(output)
        })
    }
}

pub(super) fn safe_error(operation: &Operation, error: Error) -> Error {
    if error.detail::<RefusedGuard>().is_some() {
        return error;
    }
    let Some(api) = error.api_error() else {
        return error;
    };
    let overflow = error.is_body_too_large();
    let status = api.observed_status();
    if overflow && status == Some(200) {
        return invalid_response(operation).retain_status_visibility(Some(api));
    }
    if !overflow && (!api.is_for_operation(operation.id) || status == Some(200)) {
        return error;
    }
    let safe = Error::from_api(ApiError::new(
        operation.id,
        status.filter(|_| !overflow),
        SafeText::new("", &[]),
        SafeText::new("request failed; response withheld", &[]),
        api.retryable(),
    ))
    .retain_status_visibility(Some(api));
    let safe = if error.class() == ErrorClass::Api {
        safe
    } else {
        Error::new(error.class()).with_cause(safe)
    };
    safe.with_clean_absence(error.clean_absence_proof())
}

pub(super) fn invalid_response(operation: &Operation) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation.id,
        Some(200),
        SafeText::new("InvalidResponse", &[]),
        SafeText::new("invalid response; body withheld", &[]),
        false,
    ))
    .with_http_status(StatusCode::OK)
}

pub(super) const fn field(name: &'static str, shape: Shape<'static>) -> ShapeField<'static> {
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
const INSTANCE_SAFETY_FIELDS: &[&str] = &[
    "id",
    "status",
    "datastoreType",
    "zoneId",
    "projectId",
    "owner",
    "replicaSourceId",
    "replicas",
    "deleted",
];

fn canonical_keys(object: &BTreeMap<String, &RawValue>, fields: &[&str]) -> bool {
    object.keys().all(|key| {
        !fields
            .iter()
            .any(|field| *field != key && field_matches(field, key))
    })
}

impl Data for Instance {
    const SHAPE: Shape<'static> = Instance::SCHEMA;
    fn canonical_fields(raw: &RawValue) -> bool {
        serde_json::from_str(raw.get())
            .is_ok_and(|object| canonical_keys(&object, INSTANCE_SAFETY_FIELDS))
    }
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
    fn canonical_fields(raw: &RawValue) -> bool {
        let Ok(object) = serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get()) else {
            return false;
        };
        canonical_keys(&object, &["projectId", "data", "pageObject"])
            && object.get("data").is_some_and(|items| {
                serde_json::from_str::<Vec<&RawValue>>(items.get())
                    .is_ok_and(|rows| rows.into_iter().all(Instance::canonical_fields))
            })
    }
    fn valid(&self) -> bool {
        self.data.iter().all(Data::valid)
    }
}

pub(super) fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
