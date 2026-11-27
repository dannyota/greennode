use super::Channel;
use crate::model::wire::Field;
use crate::wire::strict_json::{Shape, ShapeField};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct CreateAck {
    id: Option<Field<String>>,
    name: Option<Field<String>>,
    address: Option<Field<String>>,
    header: Option<Field<String>>,
    created_date: Option<Field<String>>,
}

pub(super) const CREATED: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "address",
        shape: Shape::Any,
    },
    ShapeField {
        name: "header",
        shape: Shape::Any,
    },
    ShapeField {
        name: "createdDate",
        shape: Shape::Any,
    },
]);

impl CreateAck {
    pub(super) fn into_channel(self) -> Channel {
        let header = self.header.map(|value| value.0).unwrap_or_default();
        Channel {
            id: self.id.map(|value| value.0).unwrap_or_default(),
            name: self.name.map(|value| value.0).unwrap_or_default(),
            address: self.address.map(|value| value.0).unwrap_or_default(),
            headers: super::super::channels::decoded_headers(&header),
            raw_header: header,
            created_date: self.created_date.map(|value| value.0).unwrap_or_default(),
            ..Channel::default()
        }
    }
}
