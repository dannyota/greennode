use crate::Error;
use crate::model::wire::WireField;
use crate::quote::request::invalid;
use crate::wire::strict_json::field_matches;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

pub(crate) const PAGING: &[&str] = &[
    "page",
    "paging",
    "pagination",
    "pageSize",
    "totalPage",
    "totalPages",
    "totalItem",
    "next",
    "nextPage",
    "isNext",
    "hasMore",
    "continuationToken",
];
const ENVELOPE: &[&str] = &[
    "data",
    "datas",
    "success",
    "code",
    "errorCode",
    "errorMsg",
    "images",
    "volumeTypes",
    "volumeTypeZones",
    "flavors",
    "flavorZones",
];

pub(crate) fn exact(
    op: &'static str,
    object: &Map<String, Value>,
    fields: &[&str],
) -> Result<(), Box<Error>> {
    if object.keys().any(|name| {
        fields
            .iter()
            .any(|field| field != name && field_matches(field, name))
    }) {
        return Err(invalid(op, 200));
    }
    Ok(())
}

pub(crate) fn envelope_fields(
    op: &'static str,
    object: &Map<String, Value>,
) -> Result<(), Box<Error>> {
    exact(op, object, PAGING)?;
    exact(op, object, ENVELOPE)
}

pub(crate) fn unpaged(
    op: &'static str,
    object: &Map<String, Value>,
    allow_is_next: bool,
) -> Result<(), Box<Error>> {
    envelope_fields(op, object)?;
    if PAGING
        .iter()
        .any(|key| (!allow_is_next || *key != "isNext") && object.contains_key(*key))
    {
        return Err(invalid(op, 200));
    }
    Ok(())
}

pub(crate) fn wire_field<'de, D: Deserializer<'de>, T: WireField>(
    deserializer: D,
) -> Result<T, D::Error> {
    T::read(deserializer)
}

pub(super) fn zero_rows<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    let rows = Option::<Vec<Option<T>>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(rows.into_iter().map(Option::unwrap_or_default).collect())
}

pub(crate) fn record<T: DeserializeOwned>(
    op: &'static str,
    row: &Value,
    fields: &[&str],
) -> Result<T, Box<Error>> {
    exact(op, row.as_object().ok_or_else(|| invalid(op, 200))?, fields)?;
    let body = serde_json::to_vec(row).map_err(|_| invalid(op, 200))?;
    serde_json::from_slice(&body).map_err(|_| invalid(op, 200))
}

pub(crate) fn array_rows<'a>(
    op: &'static str,
    raw: &'a Value,
    key: Option<&str>,
) -> Result<&'a [Value], Box<Error>> {
    let rows = if let Some(key) = key {
        let obj = raw.as_object().ok_or_else(|| invalid(op, 200))?;
        unpaged(op, obj, false)?;
        exact(op, obj, &[key])?;
        if obj
            .get("success")
            .is_some_and(|value| value.as_bool() != Some(true))
        {
            return Err(invalid(op, 200));
        }
        obj.get(key).ok_or_else(|| invalid(op, 200))?
    } else {
        raw
    };
    rows.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(op, 200))
}

pub(crate) fn array<T: DeserializeOwned>(
    op: &'static str,
    raw: &Value,
    key: Option<&str>,
    fields: &[&str],
) -> Result<Vec<T>, Box<Error>> {
    array_rows(op, raw, key)?
        .iter()
        .map(|row| record(op, row, fields))
        .collect()
}
