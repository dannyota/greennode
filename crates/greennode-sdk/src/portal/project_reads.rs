use super::*;
use crate::wire::product::operation::Output;
use crate::wire::strict_json::{Shape, field_matches};
use serde::{Deserialize, Deserializer, de};

/// Selects all project zones.
#[derive(Clone, Debug, Default)]
pub struct ListZonesInput {}
/// Selects all project quota entries.
#[derive(Clone, Debug, Default)]
pub struct ListQuotaUsedInput {}
/// Selects a project quota by name.
#[derive(Clone, Debug, Default)]
pub struct GetQuotaInput {
    /// Quota name, matched without ASCII case distinctions.
    pub name: String,
}
/// Selects the project tag quota.
#[derive(Clone, Debug, Default)]
pub struct GetTagQuotaInput {}
/// Zone properties returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct Zone {
    /// Zone identifier.
    #[serde(rename(serialize = "ID"))]
    pub id: Option<String>,
    /// Zone UUID.
    #[serde(rename(serialize = "UUID"))]
    pub uuid: Option<String>,
    /// Zone name.
    #[serde(default, deserialize_with = "null_string")]
    pub name: String,
    /// Zone type.
    pub zone_type: Option<String>,
    /// Whether this zone is the default.
    pub is_default: Option<bool>,
    /// Zone description.
    pub description: Option<String>,
    /// Whether this zone is enabled.
    pub is_enabled: Option<bool>,
    /// Volume count.
    pub volume_count: Option<i64>,
    /// Server count.
    pub server_count: Option<i64>,
    /// OpenStack zone.
    #[serde(rename(deserialize = "openstackZone"))]
    pub open_stack_zone: Option<String>,
    /// IP ranges.
    #[serde(rename(serialize = "IPRanges"))]
    pub ip_ranges: Option<Vec<String>>,
}
/// A project quota with API-defined fields.
pub type Quota = UserInfo;
/// A project tag quota with API-defined fields.
pub type TagQuota = UserInfo;
/// Zones visible to the configured project.
pub type ListZonesOutput = crate::List<Zone>;
/// Quota entries for the configured project.
pub type ListQuotaUsedOutput = crate::List<Quota>;
/// One matching quota entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetQuotaOutput {
    /// The matching quota.
    #[serde(rename = "Quota")]
    pub quota: Quota,
}
/// The project tag quota.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetTagQuotaOutput {
    /// The tag quota.
    #[serde(rename = "TagQuota")]
    pub tag_quota: TagQuota,
}

fn null_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

const LIST_KEYS: &[&str] = &[
    "listData",
    "data",
    "items",
    "content",
    "results",
    "projects",
    "regions",
    "zones",
    "quotas",
    "quotaUsed",
    "volumeTypes",
    "volumeTypeZones",
];

struct Collection<T>(Vec<T>);
impl<T> Default for Collection<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Collection<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
        let text = raw.get();
        if text.trim_start().starts_with('[') {
            return serde_json::from_str(text)
                .map(Self)
                .map_err(de::Error::custom);
        }
        let fields: BTreeMap<String, &serde_json::value::RawValue> =
            serde_json::from_str(text).map_err(de::Error::custom)?;
        let groups: &[&[&str]] = &[
            &["page"],
            &["pageSize", "size"],
            &["totalPage", "totalPages"],
            &["totalItem", "totalItems", "total"],
        ];
        for group in groups {
            let mut seen = None;
            let mut names = std::collections::HashSet::new();
            for (key, raw) in &fields {
                if let Some(name) = group.iter().find(|name| field_matches(key, name)) {
                    if !names.insert(*name) {
                        return Err(de::Error::custom("invalid portal metadata"));
                    }
                    let value: i64 = serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
                    if value < 0 || seen.is_some_and(|old| old != value) {
                        return Err(de::Error::custom("invalid portal metadata"));
                    }
                    seen = Some(value);
                }
            }
        }
        let mut selected = None;
        for (key, value) in fields {
            if let Some(name) = LIST_KEYS.iter().find(|name| field_matches(&key, name))
                && (key != *name || selected.replace(value).is_some())
            {
                return Err(de::Error::custom("invalid portal list"));
            }
        }
        let raw = selected.ok_or_else(|| de::Error::custom("invalid portal list"))?;
        if !raw.get().trim_start().starts_with('[') {
            return Err(de::Error::custom("invalid portal list"));
        }
        serde_json::from_str(raw.get())
            .map(Self)
            .map_err(de::Error::custom)
    }
}
impl<T: for<'de> Deserialize<'de> + Serialize> Output for Collection<T> {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn reflected_response(
        operation: &Operation,
        response: &crate::wire::product::sender::Response,
    ) -> Box<Error> {
        Box::new(super::envelope::invalid_response(operation, response))
    }

    fn decode_response(
        operation: &Operation,
        response: &crate::wire::product::sender::Response,
    ) -> Result<Self, Box<Error>> {
        if response.status != StatusCode::OK
            || (!response.body.is_empty()
                && serde_json::from_slice::<&serde_json::value::RawValue>(&response.body).is_err())
        {
            return operation::decode_response(operation, response);
        }
        let invalid = || Self::reflected_response(operation, response);
        let output: Self =
            operation::decode_response(operation, response).map_err(|_| invalid())?;
        let evidence =
            crate::wire::product::evidence::Evidence::from_credentials(&response.credentials);
        if evidence.serialized_output_reflects(&output.0) {
            return Err(invalid());
        }
        Ok(output)
    }
}

#[derive(Serialize)]
#[serde(transparent)]
struct QuotaRow(UserInfo);

impl<'de> Deserialize<'de> for QuotaRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = UserInfo::deserialize(deserializer)?;
        for field in value.fields.values_mut() {
            normalize_numbers(field).map_err(de::Error::custom)?;
        }
        Ok(Self(value))
    }
}

fn normalize_numbers(value: &mut Value) -> Result<(), &'static str> {
    match value {
        Value::Number(number) => {
            let float = number
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or("invalid number")?;
            let text = crate::output_number::f64(float, crate::output_number::Style::Json)
                .ok_or("invalid number")?;
            *number = serde_json::Number::from_string_unchecked(text);
        }
        Value::Array(values) => {
            for value in values {
                normalize_numbers(value)?;
            }
        }
        Value::Object(fields) => {
            for value in fields.values_mut() {
                normalize_numbers(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl<'de> Deserialize<'de> for UserInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = BTreeMap::<String, &serde_json::value::RawValue>::deserialize(deserializer)?;
        Ok(Self {
            fields: super::envelope::decode_fields(fields).map_err(de::Error::custom)?,
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads zones for the configured project.
    pub async fn list_zones(
        &self,
        ctx: &RequestContext,
        _: ListZonesInput,
    ) -> Result<ListZonesOutput, Failure<ListZonesOutput>> {
        let items = self
            .project_read::<Collection<Zone>, _>(ctx, "portal.ListZones", "v1/{}/zones")
            .await?
            .0;
        Ok(crate::List { items })
    }
    /// Reads quota entries for the configured project.
    pub async fn list_quota_used(
        &self,
        ctx: &RequestContext,
        _: ListQuotaUsedInput,
    ) -> Result<ListQuotaUsedOutput, Failure<ListQuotaUsedOutput>> {
        let items = self
            .project_read::<Collection<QuotaRow>, _>(
                ctx,
                "portal.ListQuotaUsed",
                "v2/{}/quotas/quotaUsed",
            )
            .await?
            .0;
        Ok(crate::List {
            items: items.into_iter().map(|row| row.0).collect(),
        })
    }
    /// Finds the first quota entry matching the supplied name.
    pub async fn get_quota(
        &self,
        ctx: &RequestContext,
        input: GetQuotaInput,
    ) -> Result<GetQuotaOutput, Failure<GetQuotaOutput>> {
        if input.name.is_empty() {
            return Err(Failure::new(
                Error::new(ErrorClass::InvalidInput),
                None,
                WriteDispatch::NotAWrite,
            ));
        }
        let list = self
            .list_quota_used(ctx, ListQuotaUsedInput {})
            .await
            .map_err(|e| Failure::new(e.into_parts().0, None, WriteDispatch::NotAWrite))?;
        let quota = list.items.into_iter().find(|quota| {
            [
                "name",
                "quotaName",
                "resourceName",
                "resource",
                "key",
                "type",
            ]
            .iter()
            .any(|key| {
                quota
                    .fields
                    .get(*key)
                    .is_some_and(|value| match value.as_str() {
                        Some(text) => text.eq_ignore_ascii_case(&input.name),
                        None => value.to_string().eq_ignore_ascii_case(&input.name),
                    })
            })
        });
        quota.map(|quota| GetQuotaOutput { quota }).ok_or_else(|| {
            Failure::new(
                Error::new(ErrorClass::NotFound).with_detail(crate::Diagnostic::QuotaNotFound),
                None,
                WriteDispatch::NotAWrite,
            )
        })
    }
    /// Reads the project tag quota.
    pub async fn get_tag_quota(
        &self,
        ctx: &RequestContext,
        _: GetTagQuotaInput,
    ) -> Result<GetTagQuotaOutput, Failure<GetTagQuotaOutput>> {
        let envelope = self
            .project_read::<Envelope, _>(ctx, "portal.GetTagQuota", "v2/{}/tag/quota")
            .await?;
        Ok(GetTagQuotaOutput {
            tag_quota: envelope.0,
        })
    }
    async fn project_read<T: Output, O>(
        &self,
        ctx: &RequestContext,
        id: &'static str,
        template: &'static str,
    ) -> Result<T, Failure<O>> {
        let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
        let project = self
            .session
            .require_project_id(ctx)
            .await
            .map_err(|e| failure(*e))?;
        let base = self
            .session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut operation = user_info_operation();
        operation.id = id;
        operation.template = template;
        let deadline = self.session.clock().monotonic_now() + Duration::from_secs(30);
        let ctx = ctx.child_with_deadline(deadline);
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(&ctx, &mut jitter)
            .map_err(|e| failure(*e))?;
        operation::execute::<T>(
            runtime,
            base,
            &operation,
            &[&project],
            &[],
            self.session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, output)| output)
        .map_err(|error| failure(withheld(&operation, error.into_parts().0)))
    }
}
