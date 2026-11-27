use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use super::models::{Field, Resource, array, null_default, null_items, redact_integer, scalar};
use crate::Error;
use crate::wire::product::{
    operation::{self, Operation, Output},
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};

/// The failure found while decoding a successful DNS response.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The response body was not valid JSON.
    MalformedJson,
    /// The JSON did not conform to the DNS response schema.
    InvalidShape,
}

crate::error::debug_record!(DecodeError; MalformedJson, InvalidShape);

/// A single page of DNS resources with redaction-aware pagination metadata.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, bound(deserialize = "T: Default + Deserialize<'de>"))]
pub struct Page<T> {
    /// Resources returned in this page.
    #[serde(
        rename(serialize = "Items", deserialize = "listData"),
        deserialize_with = "null_items"
    )]
    pub items: Vec<T>,
    /// One-based page number.
    #[serde(
        rename(serialize = "Page", deserialize = "page"),
        deserialize_with = "null_default"
    )]
    pub page: Field<i64>,
    /// Number of items per page reported by the service.
    #[serde(
        rename(serialize = "PageSize", deserialize = "pageSize"),
        deserialize_with = "null_default"
    )]
    pub page_size: Field<i64>,
    /// Total number of pages reported by the service.
    #[serde(
        rename(serialize = "TotalPage", deserialize = "totalPage"),
        deserialize_with = "null_default"
    )]
    pub total_page: Field<i64>,
    /// Total number of items reported by the service.
    #[serde(
        rename(serialize = "TotalItem", deserialize = "totalItem"),
        deserialize_with = "null_default"
    )]
    pub total_item: Field<i64>,
}

crate::wire::product::output::public_fields!(Page<T> { items, page, page_size, total_page, total_item });

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(super) struct PageResponse<T: DeserializeOwned + Resource> {
    #[serde(bound(deserialize = "T: DeserializeOwned"))]
    pub(super) page: Page<T>,
    #[serde(skip)]
    pub(super) incomplete: bool,
}

impl<T: DeserializeOwned + Resource> Output for PageResponse<T> {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        array("listData", &T::SHAPE),
        scalar("page"),
        scalar("pageSize"),
        scalar("totalPage"),
        scalar("totalItem"),
    ]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let mut decoded: Self = decode(operation, response, false)?;
        let page = &mut decoded.page;
        decoded.incomplete = matches!(page.total_page, Field::Value(total) if total > 1)
            || matches!(page.total_item, Field::Value(total) if total > page.items.len() as i64);
        for value in [
            &mut page.page,
            &mut page.page_size,
            &mut page.total_page,
            &mut page.total_item,
        ] {
            redact_integer(value, &response.credentials);
        }
        for item in &mut page.items {
            item.redact(&response.credentials);
        }
        Ok(decoded)
    }
}

#[derive(Default, Deserialize)]
#[serde(default, bound(deserialize = "T: DeserializeOwned"))]
pub(super) struct Detail<T: DeserializeOwned + Resource> {
    #[serde(deserialize_with = "null_default")]
    pub(super) data: T,
}

impl<T: DeserializeOwned + Resource> Output for Detail<T> {
    const SCHEMA: Shape<'static> = Shape::Object(&[ShapeField {
        name: "data",
        shape: T::SHAPE,
    }]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let mut decoded: Self = decode(operation, response, false)?;
        decoded.data.redact(&response.credentials);
        Ok(decoded)
    }
}

pub(super) fn decode<T: Output>(
    operation: &Operation,
    response: &Response,
    status_code_fallback: bool,
) -> Result<T, Box<Error>> {
    if !operation.success.contains(&response.status) {
        return crate::wire::product::response::classify(
            operation.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            operation.success,
            (status_code_fallback, operation.sensitive, operation.retry),
            &response.credentials,
        )
        .map(|_| T::default());
    }
    if (operation.allow_empty && response.body.is_empty())
        || (response.body.trim_ascii() == b"null"
            && serde_json::from_slice::<()>(&response.body).is_ok())
    {
        return Ok(T::default());
    }
    operation::decode_response(operation, response).map_err(|_| {
        let detail = if serde_json::from_slice::<&RawValue>(&response.body).is_err() {
            DecodeError::MalformedJson
        } else {
            DecodeError::InvalidShape
        };
        Box::new(
            super::api_error(operation.id, "", "response failed to decode").with_detail(detail),
        )
    })
}
