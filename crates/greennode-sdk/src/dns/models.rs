use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Date, Month, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::wire::product::sender::SentCredentials;
use crate::wire::strict_json::{Shape, ShapeField};

const REDACTED: &str = "<redacted>";

/// A typed DNS field whose rendered value can be redacted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field<T> {
    /// The original value did not contain a credential sent for the request.
    Value(T),
    /// The original value was removed and projects to `<redacted>`.
    Redacted,
}

impl<T: Default> Default for Field<T> {
    fn default() -> Self {
        Self::Value(T::default())
    }
}

impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Redacted => serializer.serialize_str(REDACTED),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Value)
    }
}

impl<T> Field<T> {
    pub(super) fn redact_if(&mut self, matches: impl FnOnce(&T) -> bool) {
        if let Self::Value(value) = self
            && matches(value)
        {
            *self = Self::Redacted;
        }
    }
}

/// A DNS timestamp with its original offset and nanosecond precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(#[serde(with = "rfc3339")] pub OffsetDateTime);

impl Default for Timestamp {
    fn default() -> Self {
        Self(
            Date::from_calendar_date(1, Month::January, 1)
                .map_or(OffsetDateTime::UNIX_EPOCH, |date| {
                    date.midnight().assume_utc()
                }),
        )
    }
}

mod rfc3339 {
    use super::*;

    pub(super) fn serialize<S: Serializer>(
        value: &OffsetDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let text = value.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)
    }
}

/// A VPC association returned with a hosted zone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct VpcMapRegion {
    /// Associated VPC identifier.
    #[serde(rename(serialize = "VPCID"), deserialize_with = "null_default")]
    pub vpc_id: String,
    /// Region of the associated VPC.
    #[serde(deserialize_with = "null_default")]
    pub region: String,
}

/// A hosted DNS zone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct HostedZone {
    /// Hosted zone identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "hostedZoneId"),
        deserialize_with = "null_default"
    )]
    pub id: String,
    /// Zone domain name.
    #[serde(deserialize_with = "null_default")]
    pub domain_name: String,
    /// Zone status.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    /// Zone description.
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    /// Zone type.
    #[serde(
        rename(serialize = "Type", deserialize = "type"),
        deserialize_with = "null_default"
    )]
    pub zone_type: String,
    /// Number of records in the zone.
    #[serde(deserialize_with = "null_default")]
    pub count_records: Field<i64>,
    /// Associated VPC identifiers.
    #[serde(
        rename(serialize = "AssociatedVPCIDs", deserialize = "assocVpcIds"),
        deserialize_with = "null_items"
    )]
    pub associated_vpc_ids: Vec<String>,
    /// Associated VPC identifiers with their regions.
    #[serde(
        rename(serialize = "AssocVPCMapRegion"),
        deserialize_with = "null_items"
    )]
    pub assoc_vpc_map_region: Vec<VpcMapRegion>,
    /// Portal account identifier.
    #[serde(rename(serialize = "PortalUserID"), deserialize_with = "null_default")]
    pub portal_user_id: Field<i64>,
    /// Zone creation time.
    #[serde(deserialize_with = "null_default")]
    pub created_at: Field<Timestamp>,
    /// Zone deletion time, when present.
    pub deleted_at: Option<Field<Timestamp>>,
    /// Last zone update time.
    #[serde(deserialize_with = "null_default")]
    pub updated_at: Field<Timestamp>,
}

/// One value in a DNS record's routing policy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all(serialize = "PascalCase"))]
pub struct RecordValue {
    /// The record data.
    #[serde(deserialize_with = "null_default")]
    pub value: String,
    /// Optional routing location.
    pub location: Option<String>,
    /// Optional routing weight.
    pub weight: Option<Field<i64>>,
}

/// A record in a hosted DNS zone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct Record {
    /// Record identifier.
    #[serde(
        rename(serialize = "ID", deserialize = "recordId"),
        deserialize_with = "null_default"
    )]
    pub id: String,
    /// Record name.
    #[serde(
        rename(serialize = "SubDomain", deserialize = "subDomain"),
        deserialize_with = "null_default"
    )]
    pub sub_domain: String,
    /// Parent hosted zone identifier.
    #[serde(rename(serialize = "HostedZoneID"), deserialize_with = "null_default")]
    pub hosted_zone_id: String,
    /// Record status.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    /// Record type, such as `A`, `CNAME`, or `TXT`.
    #[serde(
        rename(serialize = "Type", deserialize = "type"),
        deserialize_with = "null_default"
    )]
    pub record_type: String,
    /// Routing policy.
    #[serde(deserialize_with = "null_default")]
    pub routing_policy: String,
    /// Values returned for this record.
    #[serde(deserialize_with = "null_items")]
    pub value: Vec<RecordValue>,
    /// Record time to live in seconds.
    #[serde(rename(serialize = "TTL"), deserialize_with = "null_default")]
    pub ttl: Field<i64>,
    /// Sticky-session setting, when supplied by the service.
    pub enable_sticky_session: Option<Field<bool>>,
    /// Record creation time.
    #[serde(deserialize_with = "null_default")]
    pub created_at: Field<Timestamp>,
    /// Record deletion time, when present.
    pub deleted_at: Option<Field<Timestamp>>,
    /// Last record update time.
    #[serde(deserialize_with = "null_default")]
    pub updated_at: Field<Timestamp>,
}

pub(super) trait Resource: Default {
    const SHAPE: Shape<'static>;

    fn redact(&mut self, credentials: &SentCredentials);
    fn redact_ids(&mut self, keep: impl Fn(&str) -> bool);
}

impl Resource for HostedZone {
    const SHAPE: Shape<'static> = Shape::Object(&[
        scalar("hostedZoneId"),
        scalar("domainName"),
        scalar("status"),
        scalar("description"),
        scalar("type"),
        scalar("countRecords"),
        array("assocVpcIds", &Shape::Any),
        array(
            "assocVpcMapRegion",
            &Shape::Object(&[scalar("vpcId"), scalar("region")]),
        ),
        scalar("portalUserId"),
        scalar("createdAt"),
        scalar("deletedAt"),
        scalar("updatedAt"),
    ]);

    fn redact(&mut self, credentials: &SentCredentials) {
        for value in [
            &mut self.id,
            &mut self.domain_name,
            &mut self.status,
            &mut self.description,
            &mut self.zone_type,
        ]
        .into_iter()
        .chain(&mut self.associated_vpc_ids)
        {
            redact_string(value, credentials);
        }
        for association in &mut self.assoc_vpc_map_region {
            redact_id(&mut association.vpc_id, &|id| !credentials.id_reflects(id));
            redact_string(&mut association.region, credentials);
        }
        for value in [&mut self.count_records, &mut self.portal_user_id] {
            redact_integer(value, credentials);
        }
        redact_times(
            &mut self.created_at,
            &mut self.deleted_at,
            &mut self.updated_at,
            credentials,
        );
    }

    fn redact_ids(&mut self, keep: impl Fn(&str) -> bool) {
        for id in std::iter::once(&mut self.id)
            .chain(&mut self.associated_vpc_ids)
            .chain(
                self.assoc_vpc_map_region
                    .iter_mut()
                    .map(|association| &mut association.vpc_id),
            )
        {
            redact_id(id, &keep);
        }
    }
}

impl Resource for Record {
    const SHAPE: Shape<'static> = Shape::Object(&[
        scalar("recordId"),
        scalar("subDomain"),
        scalar("hostedZoneId"),
        scalar("status"),
        scalar("type"),
        scalar("routingPolicy"),
        array(
            "value",
            &Shape::Object(&[scalar("value"), scalar("location"), scalar("weight")]),
        ),
        scalar("ttl"),
        scalar("enableStickySession"),
        scalar("createdAt"),
        scalar("deletedAt"),
        scalar("updatedAt"),
    ]);

    fn redact(&mut self, credentials: &SentCredentials) {
        for value in [
            &mut self.id,
            &mut self.sub_domain,
            &mut self.hosted_zone_id,
            &mut self.status,
            &mut self.record_type,
            &mut self.routing_policy,
        ] {
            redact_string(value, credentials);
        }
        for item in &mut self.value {
            redact_string(&mut item.value, credentials);
            if let Some(location) = &mut item.location {
                redact_string(location, credentials);
            }
            if let Some(weight) = &mut item.weight {
                redact_integer(weight, credentials);
            }
        }
        redact_integer(&mut self.ttl, credentials);
        if let Some(sticky) = &mut self.enable_sticky_session {
            sticky.redact_if(|value| {
                credentials.text_reflects(if *value { "true" } else { "false" })
            });
        }
        redact_times(
            &mut self.created_at,
            &mut self.deleted_at,
            &mut self.updated_at,
            credentials,
        );
    }

    fn redact_ids(&mut self, keep: impl Fn(&str) -> bool) {
        for id in [&mut self.id, &mut self.hosted_zone_id] {
            redact_id(id, &keep);
        }
    }
}

fn redact_id(id: &mut String, keep: &impl Fn(&str) -> bool) {
    if !id.is_empty() && !keep(id) {
        *id = REDACTED.into();
    }
}

fn redact_string(value: &mut String, credentials: &SentCredentials) {
    if credentials.text_reflects(value) {
        *value = REDACTED.into();
    }
}

pub(super) fn redact_integer(value: &mut Field<i64>, credentials: &SentCredentials) {
    value.redact_if(|value| credentials.integer_output_reflects(*value));
}

fn redact_times(
    created_at: &mut Field<Timestamp>,
    deleted_at: &mut Option<Field<Timestamp>>,
    updated_at: &mut Field<Timestamp>,
    credentials: &SentCredentials,
) {
    for value in [created_at, updated_at]
        .into_iter()
        .chain(deleted_at.as_mut())
    {
        value.redact_if(|value| {
            value
                .0
                .format(&Rfc3339)
                .map_or(true, |text| credentials.text_reflects(&text))
        });
    }
}

pub(super) const fn scalar(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}

pub(super) const fn array(
    name: &'static str,
    item: &'static Shape<'static>,
) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Array(item),
    }
}

pub(super) fn null_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    Option::deserialize(deserializer).map(Option::unwrap_or_default)
}

pub(super) fn null_items<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    let items: Vec<Option<T>> = null_default(deserializer)?;
    Ok(items.into_iter().map(Option::unwrap_or_default).collect())
}
