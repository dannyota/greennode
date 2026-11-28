use crate::wire::strict_json::FieldNames;
use serde::Serialize;
use serde_json::value::RawValue;

use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, Failure, List, PagedList, RequestContext};

use super::wire::{DETAIL_SHAPE, FromWire, Object, PAGE_SHAPE, Redact, ShapeResult, api, screen};
use super::{Client, Read, Volume, output, page_query, path_id};

const VOLUMES: Read = Read::vserver("volume.ListVolumes", "v2/{}/volumes", PAGE_SHAPE);
pub(super) const VOLUME: Read = Read::vserver("volume.GetVolume", "v2/{}/volumes/{}", DETAIL_SHAPE);
const UNDERLYING: Read = Read::vserver(
    "volume.GetUnderlyingVolume",
    "v2/{}/volumes/{}/mapping",
    DETAIL_SHAPE,
);
const BY_SERVER: Read = Read::vserver(
    "volume.ListVolumesByServer",
    "v2/{}/volumes/servers/{}",
    "invalid response shape; body withheld",
);

/// Name filter and page selection for the volume inventory.
#[derive(Clone, Default)]
pub struct ListVolumesInput {
    /// Volume name filter.
    pub name: String,
    /// One-based page number. A nonpositive value selects the default.
    pub page: i64,
    /// Requested page size. A nonpositive value selects the default.
    pub size: i64,
}

crate::error::debug_record!(ListVolumesInput { name, page, size });

/// Indicates that the service did not return a volume status.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UnexpectedStatus;

crate::error::debug_record!(UnexpectedStatus unit);

/// One page of volumes.
pub type ListVolumesOutput = PagedList<Volume>;

/// Identifies a volume to read.
#[derive(Clone, Default)]
pub struct GetVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
}

crate::error::debug_record!(GetVolumeInput { volume_id });

/// The requested volume.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetVolumeOutput {
    /// Current volume details.
    #[serde(rename = "Volume")]
    pub volume: Volume,
}

crate::wire::product::output::public_fields!(GetVolumeOutput { volume });

/// Identifies a volume whose backing volume should be read.
#[derive(Clone, Default)]
pub struct GetUnderlyingVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
}

crate::error::debug_record!(GetUnderlyingVolumeInput { volume_id });

/// The requested backing volume.
pub type GetUnderlyingVolumeOutput = GetVolumeOutput;

/// Identifies the server whose attached volumes should be read.
#[derive(Clone, Default)]
pub struct ListVolumesByServerInput {
    /// Server UUID.
    pub server_id: String,
}

crate::error::debug_record!(ListVolumesByServerInput { server_id });

/// Volumes attached to the server, including the boot volume.
pub type ListVolumesByServerOutput = List<Volume>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads the selected page of volumes in the configured project.
    pub async fn list_volumes(
        &self,
        ctx: &RequestContext,
        input: ListVolumesInput,
    ) -> Result<ListVolumesOutput, Failure<ListVolumesOutput>> {
        crate::wire::product::exit::finish!(async {
            output(self.volumes(ctx, &input, &mut Evidence::new()).await)
        })
    }

    pub(super) async fn volumes(
        &self,
        ctx: &RequestContext,
        input: &ListVolumesInput,
        evidence: &mut Evidence,
    ) -> Result<ListVolumesOutput, Box<Error>> {
        let project = self.project(ctx, evidence).await?;
        let (page, size) = page_query(input.page, input.size);
        self.read(
            ctx,
            VOLUMES,
            &[&project],
            &[("name", &input.name), ("page", &page), ("size", &size)],
            evidence,
            |raw, names| paged(raw, names, "listData", "totalPage", "totalItem"),
        )
        .await
    }

    /// Reads one volume and verifies its UUID and presence of a status.
    pub async fn get_volume(
        &self,
        ctx: &RequestContext,
        input: GetVolumeInput,
    ) -> Result<GetVolumeOutput, Failure<GetVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            output(
                self.get_volume_with_evidence(ctx, &input.volume_id, &mut Evidence::new())
                    .await,
            )
        })
    }

    pub(crate) async fn get_volume_with_evidence(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<GetVolumeOutput, Box<Error>> {
        let mut volume = self.volume(ctx, id, evidence).await?;
        volume.redact(evidence);
        screen(evidence, GetVolumeOutput { volume }, VOLUME.id)
    }

    pub(super) async fn volume(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Volume, Box<Error>> {
        self.volume_detail(ctx, id, evidence)
            .await
            .map(|(volume, _)| volume)
    }

    pub(super) async fn volume_detail(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<(Volume, bool), Box<Error>> {
        path_id(VOLUME.id, "VolumeID", id)?;
        let project = self.project(ctx, evidence).await?;
        let (volume, attachment_known) = self
            .read_value(ctx, VOLUME, &[&project, id], &[], evidence, |raw, names| {
                let raw = Object::parse(raw, names)?
                    .get("data")?
                    .ok_or(super::wire::ShapeIssue::Invalid)?;
                let volume = Volume::from_wire(raw, names)?;
                let fields = Object::parse(raw, names)?;
                let attachment_known = fields.entry("serverId")?.is_some()
                    && volume.server_id.is_empty()
                    && volume.server_id_list.as_ref().is_some_and(Vec::is_empty)
                    && volume.server_name_list.as_ref().is_none_or(Vec::is_empty);
                if volume.uuid != id {
                    return Err(super::wire::ShapeIssue::Invalid);
                }
                Ok((volume, attachment_known))
            })
            .await?;
        if volume.status.trim().is_empty() {
            return Err(Box::new(
                api(
                    VOLUME.id,
                    Some(200),
                    "InvalidResponse",
                    "volume status is missing; body withheld",
                )
                .with_detail(UnexpectedStatus),
            ));
        }
        if matches!(evidence.field_names(), FieldNames::Canonical) {
            evidence.keep_decoded_operands(VOLUME.id, &volume)?;
        }
        Ok((volume, attachment_known))
    }

    /// Reads the backing volume mapping.
    pub async fn get_underlying_volume(
        &self,
        ctx: &RequestContext,
        input: GetUnderlyingVolumeInput,
    ) -> Result<GetUnderlyingVolumeOutput, Failure<GetUnderlyingVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            output(
                async {
                    path_id(UNDERLYING.id, "VolumeID", &input.volume_id)?;
                    let mut evidence = Evidence::new();
                    let project = self.project(ctx, &mut evidence).await?;
                    let mut volume = self
                        .read_value(
                            ctx,
                            UNDERLYING,
                            &[&project, &input.volume_id],
                            &[],
                            &mut evidence,
                            Volume::from_wire,
                        )
                        .await?;
                    volume.redact(&evidence);
                    screen(&evidence, GetVolumeOutput { volume }, UNDERLYING.id)
                }
                .await,
            )
        })
    }

    /// Lists the volumes attached to the specified server, including its boot volume.
    pub async fn list_volumes_by_server(
        &self,
        ctx: &RequestContext,
        input: ListVolumesByServerInput,
    ) -> Result<ListVolumesByServerOutput, Failure<ListVolumesByServerOutput>> {
        crate::wire::product::exit::finish!(async {
            output(
                self.list_volumes_by_server_with_evidence(
                    ctx,
                    &input.server_id,
                    &mut Evidence::new(),
                )
                .await,
            )
        })
    }

    pub(crate) async fn list_volumes_by_server_with_evidence(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<ListVolumesByServerOutput, Box<Error>> {
        path_id(BY_SERVER.id, "ServerID", id)?;
        let project = self.project(ctx, evidence).await?;
        self.read(
            ctx,
            BY_SERVER,
            &[&project, id],
            &[],
            evidence,
            |raw, names| {
                let items = if raw.get().starts_with('[') {
                    Vec::from_wire(raw, names)?
                } else if raw.get() == "null" {
                    Vec::new()
                } else {
                    let values = Object::parse(raw, names)?;
                    let volumes = values.optional("volumes")?;
                    let data = values.optional("data")?;
                    let list_data = values.optional("listData")?;
                    volumes.or(data).or(list_data).unwrap_or_default()
                };
                Ok(List { items })
            },
        )
        .await
    }
}

pub(super) fn paged<T: FromWire + Default>(
    raw: &RawValue,
    names: FieldNames,
    items: &str,
    total_page: &str,
    total_item: &str,
) -> ShapeResult<PagedList<T>> {
    let fields = Object::parse(raw, names)?;
    Ok(PagedList {
        items: fields.require(items)?,
        page: fields.require("page")?,
        page_size: fields.require("pageSize")?,
        total_page: fields.require(total_page)?,
        total_item: fields.require(total_item)?,
    })
}

pub(super) fn response_id(id: &str) -> bool {
    PathIds::WithUnderscore.allows(id)
}
