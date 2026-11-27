use super::wire::{Family, Read};
use crate::model::wire::{Field, WireField, decode_lenient};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField, validate_exact_shape};
use crate::{Error, PagedList};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;

pub(super) const fn plain(id: &'static str, route: &'static str) -> Read {
    Read {
        id,
        route,
        limit: 8 << 20,
        family: Family::Plain { strict: false },
    }
}

pub(super) const fn strict(id: &'static str, route: &'static str) -> Read {
    Read {
        family: Family::Plain { strict: true },
        ..plain(id, route)
    }
}

pub(super) fn output<T: DeserializeOwned>(
    raw: &RawValue,
    shape: &Shape<'_>,
    read: &Read,
    evidence: &Evidence,
) -> Result<T, Error> {
    validate_exact_shape(raw.get().as_bytes(), shape, raw.get().len())
        .map_err(|_| read.invalid(evidence))?;
    decode_lenient(|| serde_json::from_str(raw.get())).map_err(|error: serde_json::Error| {
        if error.to_string().starts_with("duplicate response field") {
            return read.invalid(evidence);
        }
        read.error(
            None,
            "",
            &format!("{}: request failed: response failed to decode", read.id),
            false,
            evidence,
        )
    })
}

pub(super) fn id(value: &mut String, evidence: &Evidence) -> bool {
    if value.is_empty() {
        return true;
    }
    if !PathIds::WithUnderscore.allows(value) {
        return false;
    }
    if evidence.id_reflects(value) {
        *value = "<redacted>".into();
    }
    !evidence.id_reflects(value)
}

pub(super) fn page_values(
    page: i64,
    size: i64,
    default_page: i64,
    default_size: i64,
) -> (String, String) {
    (
        page.max(default_page).to_string(),
        if size <= 0 { default_size } else { size }.to_string(),
    )
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "T: WireField"))]
pub(super) struct Page<T> {
    #[serde(rename = "lstData", default)]
    pub(super) items: Option<Vec<Field<T>>>,
    #[serde(default)]
    page: Option<Field<i64>>,
    #[serde(default)]
    page_size: Option<Field<i64>>,
    #[serde(default)]
    total_page: Option<Field<i64>>,
    #[serde(default)]
    total_item: Option<Field<i64>>,
}

pub(super) const PAGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "lstData",
        shape: Shape::Array(&Shape::Any),
    },
    ShapeField {
        name: "page",
        shape: Shape::Any,
    },
    ShapeField {
        name: "pageSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalPage",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalItem",
        shape: Shape::Any,
    },
]);

impl<T: WireField> Page<T> {
    pub(super) fn into_page(
        self,
        require_metadata: bool,
        read: &Read,
        evidence: &Evidence,
    ) -> Result<PagedList<T>, Error> {
        if require_metadata
            && (self.items.is_none()
                || self.page.is_none()
                || self.page_size.is_none()
                || self.total_page.is_none()
                || self.total_item.is_none())
        {
            return Err(read.error(
                Some(200),
                "InvalidResponse",
                "name search was incomplete or ambiguous",
                false,
                evidence,
            ));
        }
        let mut values = [self.page, self.page_size, self.total_page, self.total_item]
            .map(|field| field.map_or(0, |field| field.0));
        if !values.iter_mut().all(|value| value.redact(evidence)) {
            return Err(read.invalid(evidence));
        }
        let [page, page_size, total_page, total_item] = values;
        Ok(PagedList {
            items: self
                .items
                .unwrap_or_default()
                .into_iter()
                .map(|item| item.0)
                .collect(),
            page,
            page_size,
            total_page,
            total_item,
        })
    }
}

macro_rules! get_input {
    ($(#[$meta:meta])* $name:ident, $(#[$field_meta:meta])* $field:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        pub struct $name { $(#[$field_meta])* pub $field: String }
        impl $name {
            /// Selects the resource by identifier.
            pub fn new(value: impl Into<String>) -> Self { Self { $field: value.into() } }
        }
    };
}
pub(super) use get_input;

macro_rules! list_input {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;
    };
}
pub(super) use list_input;
