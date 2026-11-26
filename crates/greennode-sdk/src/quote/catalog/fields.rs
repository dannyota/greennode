use crate::Error;
use crate::quote::request::invalid;
use crate::wire::strict_json::field_matches;
use serde::de::DeserializeOwned;
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

pub(crate) fn unpaged(
    op: &'static str,
    object: &Map<String, Value>,
    allow_is_next: bool,
) -> Result<(), Box<Error>> {
    exact(op, object, PAGING)?;
    exact(op, object, ENVELOPE)?;
    if PAGING
        .iter()
        .any(|key| (!allow_is_next || *key != "isNext") && object.contains_key(*key))
    {
        return Err(invalid(op, 200));
    }
    Ok(())
}

pub(crate) fn array<T: DeserializeOwned>(
    op: &'static str,
    raw: &Value,
    key: Option<&str>,
    fields: &[&str],
) -> Result<Vec<T>, Box<Error>> {
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
    let rows = rows.as_array().ok_or_else(|| invalid(op, 200))?;
    rows.iter()
        .map(|row| {
            exact(op, row.as_object().ok_or_else(|| invalid(op, 200))?, fields)?;
            serde_json::from_value(row.clone()).map_err(|_| invalid(op, 200))
        })
        .collect()
}
