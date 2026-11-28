use serde::Serialize;

use crate::wire::product::evidence::SentCredentials;
use crate::{Error, ErrorClass};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum FilterField {
    VpcId,
    Uuid,
    EndpointServiceId,
    ZoneUuid,
    Status,
    ZoneType,
    IsEndpointUsable,
}

impl FilterField {
    const fn is_identifier(self) -> bool {
        match self {
            Self::VpcId | Self::Uuid | Self::EndpointServiceId | Self::ZoneUuid => true,
            Self::Status | Self::ZoneType | Self::IsEndpointUsable => false,
        }
    }
}

#[derive(Serialize)]
pub(super) struct Filter<'a> {
    field: FilterField,
    pub(super) value: &'a str,
}

impl<'a> Filter<'a> {
    pub(super) const fn new(field: FilterField, value: &'a str) -> Self {
        Self { field, value }
    }

    pub(super) fn check(filters: &[Self], credentials: &SentCredentials) -> Option<Box<Error>> {
        filters
            .iter()
            .any(|filter| filter.field.is_identifier() && credentials.id_reflects(filter.value))
            .then(|| Box::new(Error::new(ErrorClass::InvalidInput)))
    }
}
