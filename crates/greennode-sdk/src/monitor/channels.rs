use super::Client;
use super::inventory::{PAGE, Page, get_input, id, list_input, output, page_values, plain};
use super::wire::{failure, requires_failure};
use crate::model::wire::{Field, WireField, go_json, model};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField, validate};
use crate::{Error, ErrorClass, Failure, List, PagedList, RequestContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

const LIST: super::wire::Read = plain(
    "monitor.ListChannels",
    "notification-gateway/api/v1/notification/list/typeSearch",
);
const TYPES: super::wire::Read = plain(
    "monitor.ListChannelTypes",
    "notification-gateway/api/v1/type/list",
);
const TYPE_LIST: Shape<'static> = Shape::Object(&[ShapeField {
    name: "lstData",
    shape: Shape::Array(&Shape::Any),
}]);
const NOTIFICATION_TYPE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "description",
        shape: Shape::Any,
    },
]);
const GUARD_CHANNEL: Shape<'static> = Shape::Object(&[
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
        name: "typeNotification",
        shape: NOTIFICATION_TYPE,
    },
    ShapeField {
        name: "metricMappingId",
        shape: Shape::Any,
    },
    ShapeField {
        name: "createdDate",
        shape: Shape::Any,
    },
    ShapeField {
        name: "updatedDate",
        shape: Shape::Any,
    },
]);
const GUARD_PAGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "lstData",
        shape: Shape::Array(&GUARD_CHANNEL),
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
const REDACTED: &str = "<redacted>";

get_input!(
    /// Selects the channel to locate across pages.
    GetChannelInput,
    /// Channel identifier.
    channel_id
);
list_input!(
    /// Selects the available channel types.
    ListChannelTypesInput
);

/// Filters and pagination for notification channels.
#[derive(Clone, Debug, Default)]
pub struct ListChannelsInput {
    /// Channel type filter. Empty selects every type.
    pub r#type: String,
    /// One-based page. Non-positive values select the first page.
    pub page: i64,
    /// Requested page size. A non-positive value selects 10000.
    pub size: i64,
}
/// One page of notification channels.
pub type ListChannelsOutput = PagedList<Channel>;
/// Channel types available to the account.
pub type ListChannelTypesOutput = List<ChannelType>;

/// Selected notification channel.
#[derive(Clone, Debug, Serialize)]
pub struct GetChannelOutput {
    /// The matching channel.
    #[serde(rename = "Channel")]
    pub channel: Channel,
}

model! { @ true;
    /// A notification delivery type.
    pub struct ChannelType {
        /// Type identifier.
        id: String => "id", "ID";
        /// Type name.
        name: String => "name", "Name";
        /// Type description.
        description: String => "description", "Description";
    }
}

/// Header attached to a webhook notification.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChannelHeader {
    /// Header name.
    pub key: String,
    /// Header value. May contain channel credentials.
    pub value: String,
}

/// Account notification channel.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Channel {
    /// Channel identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Channel name.
    pub name: String,
    /// Delivery address. May contain channel credentials.
    pub address: String,
    /// Provider delivery type.
    pub r#type: String,
    /// Configured headers. Values may contain channel credentials.
    pub headers: Vec<ChannelHeader>,
    /// Identifier used by metric alarms.
    #[serde(rename = "MetricMappingID")]
    pub metric_mapping_id: String,
    /// Provider creation timestamp text.
    pub created_date: String,
    /// Provider update timestamp text.
    pub updated_date: String,
    #[serde(skip)]
    pub(super) masked_address: String,
    #[serde(skip)]
    pub(super) raw_header: String,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("id", &self.id)
            .field("type", &self.r#type)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for ChannelHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelHeader")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

model! { @ true; struct ChannelWire {
    id: String => "id", "ID";
    name: String => "name", "Name";
    address: String => "address", "Address";
    notification: ChannelType => "typeNotification", "Type";
    header: String => "header", "Header";
    mapping: String => "metricMappingId", "MetricMappingID";
    created: String => "createdDate", "CreatedDate";
    updated: String => "updatedDate", "UpdatedDate";
}}
model! { @ true; struct HeaderWire {key:String=>"key","Key"; value:String=>"value","Value";} }
model! { @ true; struct TypesWire {items:Vec<ChannelType> =>"lstData","Items";} }

impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = ChannelWire::deserialize(d)?;
        let headers = decoded_headers(&wire.header);
        Ok(Self {
            id: wire.id,
            name: wire.name,
            address: wire.address,
            r#type: wire.notification.name,
            headers,
            metric_mapping_id: wire.mapping,
            created_date: wire.created,
            updated_date: wire.updated,
            masked_address: String::new(),
            raw_header: wire.header,
        })
    }
}

pub(super) fn decoded_headers(raw: &str) -> Vec<ChannelHeader> {
    let header = go_json(raw.as_bytes());
    if validate(header.as_bytes(), &[], header.len()).is_ok() {
        serde_json::from_str::<Option<Vec<HeaderWire>>>(&header)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|header| ChannelHeader {
                key: header.key,
                value: header.value,
            })
            .collect()
    } else {
        Vec::new()
    }
}

impl Channel {
    /// Returns this channel with addresses and headers masked for presentation.
    pub fn for_display(mut self) -> Self {
        if !matches!(self.r#type.as_str(), "Email" | "SMS" | "Telegram") {
            self.address = if self.masked_address.is_empty() {
                REDACTED.into()
            } else {
                std::mem::take(&mut self.masked_address)
            };
        }
        self.raw_header.clear();
        for header in &mut self.headers {
            header.value = REDACTED.into();
        }
        self
    }
}

impl WireField for Channel {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        let safe = id(&mut self.id, evidence)
            && id(&mut self.metric_mapping_id, evidence)
            && [
                &mut self.name,
                &mut self.address,
                &mut self.r#type,
                &mut self.created_date,
                &mut self.updated_date,
            ]
            .into_iter()
            .all(|value| value.redact(evidence))
            && self
                .headers
                .iter_mut()
                .all(|header| header.key.redact(evidence) && header.value.redact(evidence));
        self.masked_address = mask_address(&self.address).unwrap_or_else(|| REDACTED.into());
        safe && !evidence.text_reflects(REDACTED) && self.masked_address.redact(evidence)
    }
}
fn mask_address(address: &str) -> Option<String> {
    let parsed = url::Url::parse(address).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return None;
    }
    let (_, rest) = address.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!("{}://{authority}/{REDACTED}", parsed.scheme()))
}

impl Client {
    /// Lists provider-supported delivery types.
    pub async fn list_channel_types(
        &self,
        ctx: &RequestContext,
        _: ListChannelTypesInput,
    ) -> Result<ListChannelTypesOutput, Failure<ListChannelTypesOutput>> {
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &TYPES, &[], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let Field(mut wire): Field<TypesWire> =
            output(&raw, &TYPE_LIST, &TYPES, &evidence).map_err(failure)?;
        if !wire
            .items
            .iter_mut()
            .all(|item| id(&mut item.id, &evidence) && item.redact(&evidence))
        {
            return Err(failure(TYPES.invalid(&evidence)));
        }
        Ok(List { items: wire.items })
    }
    /// Lists one page of notification channels.
    pub async fn list_channels(
        &self,
        ctx: &RequestContext,
        input: ListChannelsInput,
    ) -> Result<ListChannelsOutput, Failure<ListChannelsOutput>> {
        let mut evidence = Evidence::new();
        let mut page = self
            .channel_page(ctx, &input, &mut evidence, false)
            .await
            .map_err(failure)?;
        if !page.items.redact(&evidence) {
            return Err(failure(LIST.invalid(&evidence)));
        }
        Ok(page)
    }
    /// Finds a channel by identifier across the account's channel pages.
    pub async fn get_channel(
        &self,
        ctx: &RequestContext,
        input: GetChannelInput,
    ) -> Result<GetChannelOutput, Failure<GetChannelOutput>> {
        const OP: &str = "monitor.GetChannel";
        if input.channel_id.is_empty() {
            return Err(requires_failure(OP, "ChannelID"));
        }
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if evidence.id_reflects(&input.channel_id) {
            return Err(requires_failure(OP, "ChannelID"));
        }
        let mut channel = self
            .find_channel(ctx, &input.channel_id, &mut evidence, false)
            .await
            .map_err(failure)?;
        if !channel.redact(&evidence) {
            return Err(failure(LIST.invalid(&evidence)));
        }
        Ok(GetChannelOutput { channel })
    }

    pub(super) async fn find_channel(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        guard: bool,
    ) -> Result<Channel, Error> {
        let mut seen = 0_i64;
        for page in 1..=1000 {
            let list = self
                .channel_page(
                    ctx,
                    &ListChannelsInput {
                        page,
                        ..Default::default()
                    },
                    evidence,
                    guard,
                )
                .await?;
            if evidence.id_reflects(id) {
                return Err(LIST.invalid(evidence));
            }
            let count = i64::try_from(list.items.len()).unwrap_or(i64::MAX);
            if let Some(channel) = list.items.into_iter().find(|channel| channel.id == id) {
                return Ok(channel);
            }
            seen = seen.saturating_add(count);
            if count == 0 || seen >= list.total_item {
                break;
            }
        }
        let message = format!("vngcloud: not found: channel {id}");
        let message =
            crate::wire::product::response::sanitize_error_text(&message, evidence.credentials());
        Err(Error::new(ErrorClass::NotFound).with_outer_detail(message.as_str().to_owned()))
    }
    async fn channel_page(
        &self,
        ctx: &RequestContext,
        input: &ListChannelsInput,
        evidence: &mut Evidence,
        guard: bool,
    ) -> Result<ListChannelsOutput, Error> {
        let (page, size) = page_values(input.page, input.size, 1, 10000);
        let query = [
            ("type", input.r#type.as_str()),
            ("field", ""),
            ("searchtext", ""),
            ("page", page.as_str()),
            ("size", size.as_str()),
        ];
        let raw = self.read(ctx, &LIST, &[], &query, evidence).await?;
        let wire: Option<Page<Channel>> = if guard {
            super::wire::decode(&raw, &GUARD_PAGE, &LIST, evidence)?
        } else {
            output(&raw, &PAGE, &LIST, evidence)?
        };
        wire.unwrap_or_default().into_page(false, &LIST, evidence)
    }
}
