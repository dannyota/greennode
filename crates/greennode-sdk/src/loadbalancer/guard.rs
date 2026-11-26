use std::collections::BTreeMap;

use serde_json::value::RawValue;

use crate::Error;
use crate::model::wire::WireField;
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField, validate_exact_shape};

use super::wire::{DecodeError, Read};

macro_rules! shape {
    ($($key:literal),* ; $($nested:literal => $shape:expr),* $(,)?) => {
        Shape::Object(&[
            $(ShapeField { name: $key, shape: Shape::Any },)*
            $(ShapeField { name: $nested, shape: $shape },)*
        ])
    };
}

const NODE: Shape<'_> = shape!("status", "zoneId", "zoneName", "subnetId";);
const ZONE: Shape<'_> = shape!("uuid";);
pub(super) const LOAD_BALANCER: Shape<'_> = shape!(
    "uuid", "name", "displayStatus", "address", "privateSubnetId", "privateSubnetCidr",
    "type", "displayType", "loadBalancerSchema", "packageId", "description", "location",
    "createdAt", "updateAt", "progressStatus", "status", "backendSubnetId", "internal",
    "autoScalable", "zoneId", "minSize", "maxSize", "totalNodes";
    "nodes" => Shape::Array(&NODE), "zone" => ZONE,
);

pub(super) fn parse<T: WireField>(
    read: &Read,
    body: &str,
    shape: &Shape<'_>,
    evidence: &Evidence,
) -> Result<T, Box<Error>> {
    let invalid = || read.decode_error(evidence, DecodeError::InvalidShape);
    validate_exact_shape(body.as_bytes(), shape, body.len()).map_err(|_| invalid())?;
    let canonical = canonical_json(body, shape).ok_or_else(invalid)?;
    read.parse(&canonical, evidence)
}

fn canonical_json(raw: &str, shape: &Shape<'_>) -> Option<String> {
    Some(match shape {
        Shape::Any => raw.to_owned(),
        _ if raw == "null" => raw.to_owned(),
        Shape::Object(fields) => {
            let values: BTreeMap<String, &RawValue> = serde_json::from_str(raw).ok()?;
            let mut result = Vec::with_capacity(fields.len().min(values.len()));
            for field in *fields {
                if let Some(value) = values.get(field.name) {
                    let value = canonical_json(value.get(), &field.shape)?;
                    result.push(format!(
                        "{}:{value}",
                        serde_json::to_string(field.name).ok()?
                    ));
                }
            }
            format!("{{{}}}", result.join(","))
        }
        Shape::Array(item) => {
            let values: Vec<&RawValue> = serde_json::from_str(raw).ok()?;
            let rows: Vec<_> = values
                .iter()
                .map(|value| canonical_json(value.get(), item))
                .collect::<Option<_>>()?;
            format!("[{}]", rows.join(","))
        }
    })
}
