use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::super::{HostedZone, RecordValue, models::Resource, wire::decode};
use crate::Error;
use crate::wire::product::{
    operation::{Operation, Output},
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};

#[derive(Default, Deserialize)]
#[serde(default, bound(deserialize = "T: DeserializeOwned + Default"))]
pub(super) struct Detail<T> {
    pub(super) data: T,
}

impl<T: DeserializeOwned + Default + Resource> Output for Detail<Option<T>> {
    const SCHEMA: Shape<'static> = Shape::Object(&[ShapeField {
        name: "data",
        shape: T::SHAPE,
    }]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        decode(operation, response)
    }
}

#[derive(Default, Deserialize)]
pub(super) struct ZoneSnapshot {
    #[serde(flatten)]
    zone: HostedZone,
    description: Option<String>,
    #[serde(rename = "assocVpcIds")]
    vpc_ids: Option<Vec<Option<String>>>,
}

impl ZoneSnapshot {
    pub(super) fn into_zone(mut self) -> (HostedZone, bool) {
        let complete = self.description.is_some() && self.vpc_ids.is_some();
        self.zone.description = self.description.unwrap_or_default();
        self.zone.associated_vpc_ids = self
            .vpc_ids
            .unwrap_or_default()
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        (self.zone, complete)
    }
}

impl Output for Detail<Option<ZoneSnapshot>> {
    const SCHEMA: Shape<'static> = Detail::<Option<HostedZone>>::SCHEMA;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        decode(operation, response)
    }
}

#[derive(Default, Deserialize)]
pub(super) struct Acknowledgement;

impl Output for Acknowledgement {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        decode(operation, response)
    }
}

#[derive(Serialize)]
pub(super) struct RecordValueBody<'a> {
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weight: Option<i64>,
}

impl<'a> TryFrom<&'a RecordValue> for RecordValueBody<'a> {
    type Error = ();

    fn try_from(input: &'a RecordValue) -> Result<Self, Self::Error> {
        let weight = match input.weight {
            Some(super::super::Field::Value(weight)) => Some(weight),
            Some(super::super::Field::Redacted) => return Err(()),
            None => None,
        };
        Ok(Self {
            value: &input.value,
            location: input.location.as_deref(),
            weight,
        })
    }
}
