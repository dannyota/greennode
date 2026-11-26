use serde::Serialize;

use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, List, RequestContext};

use super::wire::{FromWire, Object, Redact, SNAPSHOT_SHAPE, invalid, screen};
use super::{Client, EncryptionType, Read, VolumeType, VolumeTypeZone, output, path_id};

const ZONES: Read = Read::vserver(
    "volume.ListVolumeTypeZones",
    "v1/{}/volume_type_zones",
    SNAPSHOT_SHAPE,
);
const TYPES: Read = Read::vserver(
    "volume.ListVolumeTypes",
    "v1/{}/volume_types",
    SNAPSHOT_SHAPE,
);
const TYPES_IN_ZONE: Read = Read {
    route: "v1/{}/{}/volume_types",
    ..TYPES
};
const TYPE: Read = Read::vserver(
    "volume.GetVolumeType",
    "v1/{}/volume_types/{}",
    SNAPSHOT_SHAPE,
);
const DEFAULT_TYPE: Read = Read::vserver(
    "volume.GetDefaultVolumeType",
    "v1/{}/volume_default_id",
    SNAPSHOT_SHAPE,
);
const ENCRYPTION: Read = Read::vserver(
    "volume.ListEncryptionTypes",
    "v1/{}/volumes/encryption_types",
    SNAPSHOT_SHAPE,
);

/// Optional filter for volume-type zones by network zone ID.
#[derive(Clone, Debug, Default)]
pub struct ListVolumeTypeZonesInput {
    /// Network zone used by the provider's `zoneId` filter.
    pub zone_id: String,
}

/// Volume-type zones in the project.
pub type ListVolumeTypeZonesOutput = List<VolumeTypeZone>;

/// Selects the catalog and an optional exact IOPS filter.
#[derive(Clone, Debug, Default)]
pub struct ListVolumeTypesInput {
    /// Volume-type zone to read. Mutually exclusive with `zone_id`.
    pub volume_type_zone_id: String,
    /// Network zone whose volume-type zones should be read.
    pub zone_id: String,
    /// Exact IOPS to keep. Zero disables this filter.
    pub iops: i64,
}

/// Available volume types.
pub type ListVolumeTypesOutput = List<VolumeType>;

/// Identifies a volume type to read.
#[derive(Clone, Debug, Default)]
pub struct GetVolumeTypeInput {
    /// Volume type identifier.
    pub volume_type_id: String,
}

/// The requested volume type.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetVolumeTypeOutput {
    /// Type returned by the volume catalog.
    #[serde(rename = "VolumeType")]
    pub volume_type: VolumeType,
}

/// Selects the network zone for a default volume type read.
#[derive(Clone, Debug, Default)]
pub struct GetDefaultVolumeTypeInput {
    /// Network zone identifier.
    pub zone_id: String,
}

/// The default volume type and its zone identifiers.
pub type GetDefaultVolumeTypeOutput = GetVolumeTypeOutput;

/// Input for the encryption catalog, which takes no filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListEncryptionTypesInput;

/// Encryption types offered by the project.
pub type ListEncryptionTypesOutput = List<EncryptionType>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Lists volume-type zones, optionally filtered by network zone.
    pub async fn list_volume_type_zones(
        &self,
        ctx: &RequestContext,
        input: ListVolumeTypeZonesInput,
    ) -> Result<ListVolumeTypeZonesOutput, Failure<ListVolumeTypeZonesOutput>> {
        output(
            self.type_zones(ctx, &input.zone_id, &mut Evidence::new())
                .await,
        )
    }

    async fn type_zones(
        &self,
        ctx: &RequestContext,
        zone_id: &str,
        evidence: &mut Evidence,
    ) -> Result<ListVolumeTypeZonesOutput, Box<Error>> {
        let project = self.project(ctx, evidence).await?;
        let zone = [("zoneId", zone_id)];
        let query = if zone_id.is_empty() {
            &[][..]
        } else {
            &zone[..]
        };
        self.read(ctx, ZONES, &[&project], query, evidence, |raw| {
            Ok(List {
                items: Object::parse(raw)?.value("volumeTypeZones")?,
            })
        })
        .await
    }

    /// Lists project-wide types, types in one volume-type zone, or types across a network zone.
    pub async fn list_volume_types(
        &self,
        ctx: &RequestContext,
        input: ListVolumeTypesInput,
    ) -> Result<ListVolumeTypesOutput, Failure<ListVolumeTypesOutput>> {
        output(self.types(ctx, &input).await)
    }

    async fn types(
        &self,
        ctx: &RequestContext,
        input: &ListVolumeTypesInput,
    ) -> Result<ListVolumeTypesOutput, Box<Error>> {
        if !input.zone_id.is_empty() && !input.volume_type_zone_id.is_empty() {
            return Err(invalid(
                "vngcloud: invalid input: volume.ListVolumeTypes accepts at most one of VolumeTypeZoneID and ZoneID",
            ));
        }
        if input.iops < 0 {
            return Err(invalid(
                "vngcloud: invalid input: volume.ListVolumeTypes requires IOPS to be 0 or more",
            ));
        }
        let mut evidence = Evidence::new();
        let parents = if input.zone_id.is_empty() {
            vec![input.volume_type_zone_id.clone()]
        } else {
            let rows = self.type_zones(ctx, &input.zone_id, &mut evidence).await?;
            rows.items
                .into_iter()
                .filter(|row| row.zone.uuid == input.zone_id)
                .map(|row| row.id)
                .collect()
        };
        let mut items = Vec::new();
        for parent in parents {
            if !parent.is_empty() || !input.zone_id.is_empty() {
                path_id(TYPES.id, "VolumeTypeZoneID", &parent)?;
            }
            let project = self.project(ctx, &mut evidence).await?;
            let path = [project.as_str(), &parent];
            let (read, path) = if parent.is_empty() {
                (TYPES, &path[..1])
            } else {
                (TYPES_IN_ZONE, &path[..])
            };
            let rows: Vec<VolumeType> = self
                .read(ctx, read, path, &[], &mut evidence, |raw| {
                    Object::parse(raw)?.value("volumeTypes")
                })
                .await?;
            for mut row in rows
                .into_iter()
                .filter(|row| input.iops == 0 || row.iops == input.iops)
            {
                if row.volume_type_zone_id.is_empty() {
                    row.volume_type_zone_id.clone_from(&parent);
                }
                if row.zone_id.is_empty() {
                    row.zone_id.clone_from(&input.zone_id);
                }
                items.push(row);
            }
        }
        output_screen(evidence, items)
    }

    /// Reads one volume type by identifier.
    pub async fn get_volume_type(
        &self,
        ctx: &RequestContext,
        input: GetVolumeTypeInput,
    ) -> Result<GetVolumeTypeOutput, Failure<GetVolumeTypeOutput>> {
        output(
            async {
                path_id(TYPE.id, "VolumeTypeID", &input.volume_type_id)?;
                let mut evidence = Evidence::new();
                let project = self.project(ctx, &mut evidence).await?;
                let rows: Vec<VolumeType> = self
                    .read_value(
                        ctx,
                        TYPE,
                        &[&project, &input.volume_type_id],
                        &[],
                        &mut evidence,
                        |raw| Object::parse(raw)?.value("volumeTypes"),
                    )
                    .await?;
                let mut volume_type = rows.into_iter().next().ok_or_else(|| {
                    Box::new(Error::new(ErrorClass::NotFound).with_detail(format!(
                        "vngcloud: not found: volume type {}",
                        input.volume_type_id
                    )))
                })?;
                volume_type.redact(&evidence);
                screen(&evidence, GetVolumeTypeOutput { volume_type }, TYPE.id)
            }
            .await,
        )
    }

    /// Reads the default volume type for the specified network zone.
    pub async fn get_default_volume_type(
        &self,
        ctx: &RequestContext,
        input: GetDefaultVolumeTypeInput,
    ) -> Result<GetDefaultVolumeTypeOutput, Failure<GetDefaultVolumeTypeOutput>> {
        output(async {
            if !super::inventory::response_id(&input.zone_id) {
                return Err(invalid("vngcloud: invalid input: volume.GetDefaultVolumeType requires ZoneID to match ^[A-Za-z0-9_-]+$"));
            }
            let mut evidence = Evidence::new();
            let project = self.project(ctx, &mut evidence).await?;
            let mut result: GetVolumeTypeOutput = self.read_value(ctx, DEFAULT_TYPE, &[&project], &[("zoneId", &input.zone_id)], &mut evidence, |raw| {
                let fields = Object::parse(raw)?;
                let id: String = fields.value("volumeTypeId")?;
                let zone: String = fields.value("volumeTypeZoneId")?;
                Ok(GetVolumeTypeOutput { volume_type: VolumeType {
                    id: id.clone(), volume_type_id: id, zone_id: zone.clone(), volume_type_zone_id: zone, ..VolumeType::default()
                }})
            }).await?;
            if result.volume_type.id.is_empty() {
                return Err(Box::new(Error::new(ErrorClass::NotFound).with_detail("vngcloud: not found: default volume type")));
            }
            result.volume_type.redact(&evidence);
            screen(&evidence, result, DEFAULT_TYPE.id)
        }.await)
    }

    /// Reads encryption types and normalizes IDs to their request values.
    pub async fn list_encryption_types(
        &self,
        ctx: &RequestContext,
        _input: ListEncryptionTypesInput,
    ) -> Result<ListEncryptionTypesOutput, Failure<ListEncryptionTypesOutput>> {
        output(
            async {
                let mut evidence = Evidence::new();
                let project = self.project(ctx, &mut evidence).await?;
                self.read(ctx, ENCRYPTION, &[&project], &[], &mut evidence, |raw| {
                    let array = if raw.get().starts_with('{') {
                        let object = Object::parse(raw)?;
                        object
                            .entry("encryptionTypes")?
                            .or(object.entry("data")?)
                            .or(object.entry("items")?)
                            .filter(|value| value.get() != "null")
                    } else if raw.get() == "null" {
                        None
                    } else {
                        Some(raw)
                    };
                    let items = array.map_or_else(|| Ok(Vec::new()), encryption_types)?;
                    Ok(List { items })
                })
                .await
            }
            .await,
        )
    }
}

fn output_screen(
    evidence: Evidence,
    items: Vec<VolumeType>,
) -> Result<ListVolumeTypesOutput, Box<Error>> {
    super::wire::screen(&evidence, List { items }, TYPES.id)
}

fn encryption_types(
    raw: &serde_json::value::RawValue,
) -> super::wire::ShapeResult<Vec<EncryptionType>> {
    if let Ok(mut rows) = Vec::<EncryptionType>::from_wire(raw) {
        for row in &mut rows {
            row.id = nonempty([&row.key, &row.id, &row.value, &row.display_key, &row.name]);
            row.name = nonempty([&row.name, &row.display_key, &row.id]);
            row.value = nonempty([&row.value, &row.id]);
        }
        return Ok(rows);
    }
    Vec::<String>::from_wire(raw).map(|items| {
        items
            .into_iter()
            .map(|value| EncryptionType {
                id: value.clone(),
                name: value.clone(),
                value,
                ..EncryptionType::default()
            })
            .collect()
    })
}

fn nonempty<'a>(values: impl IntoIterator<Item = &'a String>) -> String {
    values
        .into_iter()
        .find(|value| !value.is_empty())
        .cloned()
        .unwrap_or_default()
}
