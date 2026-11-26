use crate::wire::strict_json::{Shape, ShapeField};
use serde::{Deserialize, Deserializer, Serialize};

pub(super) fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

const fn field(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}

/// Database EngineVersion returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct EngineVersion {
    /// Name reported by the service.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        deserialize_with = "null_default"
    )]
    pub name: String,
    /// Type reported by the service.
    #[serde(
        rename(deserialize = "type", serialize = "Type"),
        deserialize_with = "null_default"
    )]
    pub r#type: String,
    /// VersionName reported by the service.
    #[serde(
        rename(deserialize = "versionName", serialize = "VersionName"),
        deserialize_with = "null_default"
    )]
    pub version_name: String,
    /// Version reported by the service.
    #[serde(
        rename(deserialize = "version", serialize = "Version"),
        deserialize_with = "null_default"
    )]
    pub version: String,
    /// LicenseName reported by the service.
    #[serde(
        rename(deserialize = "licenseName", serialize = "LicenseName"),
        deserialize_with = "null_default"
    )]
    pub license_name: String,
}

impl EngineVersion {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("name"),
        field("type"),
        field("versionName"),
        field("version"),
        field("licenseName"),
    ]);
}

/// Database Zone returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Zone {
    /// UUID reported by the service.
    #[serde(
        rename(deserialize = "uuid", serialize = "UUID"),
        deserialize_with = "null_default"
    )]
    pub uuid: String,
    /// Name reported by the service.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        deserialize_with = "null_default"
    )]
    pub name: String,
    /// Status reported by the service.
    #[serde(
        rename(deserialize = "status", serialize = "Status"),
        deserialize_with = "null_default"
    )]
    pub status: String,
    /// ZoneType reported by the service.
    #[serde(
        rename(deserialize = "zoneType", serialize = "ZoneType"),
        deserialize_with = "null_default"
    )]
    pub zone_type: String,
}

impl Zone {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("uuid"),
        field("name"),
        field("status"),
        field("zoneType"),
    ]);
}

/// Database Flavor returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Flavor {
    /// ID reported by the service.
    #[serde(
        rename(deserialize = "id", serialize = "ID"),
        deserialize_with = "null_default"
    )]
    pub id: i32,
    /// Name reported by the service.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        deserialize_with = "null_default"
    )]
    pub name: String,
    /// FlavorID reported by the service.
    #[serde(
        rename(deserialize = "flavorId", serialize = "FlavorID"),
        deserialize_with = "null_default"
    )]
    pub flavor_id: String,
    /// VCPUs reported by the service.
    #[serde(
        rename(deserialize = "vcpus", serialize = "VCPUs"),
        deserialize_with = "null_default"
    )]
    pub vcpus: i32,
    /// RAM reported by the service.
    #[serde(
        rename(deserialize = "ram", serialize = "RAM"),
        deserialize_with = "null_default"
    )]
    pub ram: i32,
    /// PackageSKU reported by the service.
    #[serde(
        rename(deserialize = "packageSku", serialize = "PackageSKU"),
        deserialize_with = "null_default"
    )]
    pub package_sku: String,
    /// LocateZoneID reported by the service.
    #[serde(
        rename(deserialize = "locateZoneId", serialize = "LocateZoneID"),
        deserialize_with = "null_default"
    )]
    pub locate_zone_id: String,
    /// MonthlyCost reported by the service.
    #[serde(
        rename(deserialize = "monthlyCost", serialize = "MonthlyCost"),
        deserialize_with = "null_default"
    )]
    pub monthly_cost: f64,
    /// PriceKey reported by the service.
    #[serde(
        rename(deserialize = "priceKey", serialize = "PriceKey"),
        deserialize_with = "null_default"
    )]
    pub price_key: String,
    /// VolumeSize reported by the service.
    #[serde(
        rename(deserialize = "volumeSize", serialize = "VolumeSize"),
        deserialize_with = "null_default"
    )]
    pub volume_size: i32,
    /// VolumeType reported by the service.
    #[serde(
        rename(deserialize = "volumeType", serialize = "VolumeType"),
        deserialize_with = "null_default"
    )]
    pub volume_type: String,
    /// BackupSize reported by the service.
    #[serde(
        rename(deserialize = "backupSize", serialize = "BackupSize"),
        deserialize_with = "null_default"
    )]
    pub backup_size: i32,
    /// Description reported by the service.
    #[serde(
        rename(deserialize = "description", serialize = "Description"),
        deserialize_with = "null_default"
    )]
    pub description: String,
    /// FamilyType reported by the service.
    #[serde(
        rename(deserialize = "familyType", serialize = "FamilyType"),
        deserialize_with = "null_default"
    )]
    pub family_type: String,
    /// PlatformType reported by the service.
    #[serde(
        rename(deserialize = "platformType", serialize = "PlatformType"),
        deserialize_with = "null_default"
    )]
    pub platform_type: String,
    /// Bandwidth reported by the service.
    #[serde(
        rename(deserialize = "bandwidth", serialize = "Bandwidth"),
        deserialize_with = "null_default"
    )]
    pub bandwidth: i32,
    /// BandwidthUnit reported by the service.
    #[serde(
        rename(deserialize = "bandwidthUnit", serialize = "BandwidthUnit"),
        deserialize_with = "null_default"
    )]
    pub bandwidth_unit: String,
    /// ZoneUUID reported by the service.
    #[serde(
        rename(deserialize = "zoneUUID", serialize = "ZoneUUID"),
        deserialize_with = "null_default"
    )]
    pub zone_uuid: String,
}

impl Flavor {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("id"),
        field("name"),
        field("flavorId"),
        field("vcpus"),
        field("ram"),
        field("packageSku"),
        field("locateZoneId"),
        field("monthlyCost"),
        field("priceKey"),
        field("volumeSize"),
        field("volumeType"),
        field("backupSize"),
        field("description"),
        field("familyType"),
        field("platformType"),
        field("bandwidth"),
        field("bandwidthUnit"),
        field("zoneUUID"),
    ]);
}

/// Database StorageType returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct StorageType {
    /// ID reported by the service.
    #[serde(
        rename(deserialize = "id", serialize = "ID"),
        deserialize_with = "null_default"
    )]
    pub id: i32,
    /// Type reported by the service.
    #[serde(
        rename(deserialize = "type", serialize = "Type"),
        deserialize_with = "null_default"
    )]
    pub r#type: String,
    /// MinVolumeSize reported by the service.
    #[serde(
        rename(deserialize = "minVolumeSize", serialize = "MinVolumeSize"),
        deserialize_with = "null_default"
    )]
    pub min_volume_size: i32,
    /// MaxVolumeSize reported by the service.
    #[serde(
        rename(deserialize = "maxVolumeSize", serialize = "MaxVolumeSize"),
        deserialize_with = "null_default"
    )]
    pub max_volume_size: i32,
    /// ZoneID reported by the service.
    #[serde(
        rename(deserialize = "zoneId", serialize = "ZoneID"),
        deserialize_with = "null_default"
    )]
    pub zone_id: String,
    /// VolumeTypeZoneID reported by the service.
    #[serde(
        rename(deserialize = "volumeTypeZoneId", serialize = "VolumeTypeZoneID"),
        deserialize_with = "null_default"
    )]
    pub volume_type_zone_id: String,
    /// VolumeTypeSKU reported by the service.
    #[serde(
        rename(deserialize = "volumeTypeSku", serialize = "VolumeTypeSKU"),
        deserialize_with = "null_default"
    )]
    pub volume_type_sku: String,
}

impl StorageType {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("id"),
        field("type"),
        field("minVolumeSize"),
        field("maxVolumeSize"),
        field("zoneId"),
        field("volumeTypeZoneId"),
        field("volumeTypeSku"),
    ]);
}

/// Database Subnet returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Subnet {
    /// UUID reported by the service.
    #[serde(
        rename(deserialize = "uuid", serialize = "UUID"),
        deserialize_with = "null_default"
    )]
    pub uuid: String,
    /// NetworkUUID reported by the service.
    #[serde(
        rename(deserialize = "networkUuid", serialize = "NetworkUUID"),
        deserialize_with = "null_default"
    )]
    pub network_uuid: String,
    /// Name reported by the service.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        deserialize_with = "null_default"
    )]
    pub name: String,
    /// Status reported by the service.
    #[serde(
        rename(deserialize = "status", serialize = "Status"),
        deserialize_with = "null_default"
    )]
    pub status: String,
    /// ZoneID reported by the service.
    #[serde(
        rename(deserialize = "zoneId", serialize = "ZoneID"),
        deserialize_with = "null_default"
    )]
    pub zone_id: String,
}

impl Subnet {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("uuid"),
        field("networkUuid"),
        field("name"),
        field("status"),
        field("zoneId"),
    ]);
}

/// Database Network returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Network {
    /// UUID reported by the service.
    #[serde(
        rename(deserialize = "uuid", serialize = "UUID"),
        deserialize_with = "null_default"
    )]
    pub uuid: String,
    /// DisplayName reported by the service.
    #[serde(
        rename(deserialize = "displayName", serialize = "DisplayName"),
        deserialize_with = "null_default"
    )]
    pub display_name: String,
    /// Status reported by the service.
    #[serde(
        rename(deserialize = "status", serialize = "Status"),
        deserialize_with = "null_default"
    )]
    pub status: String,
    /// ZoneID reported by the service.
    #[serde(
        rename(deserialize = "zoneId", serialize = "ZoneID"),
        deserialize_with = "null_default"
    )]
    pub zone_id: String,
    /// Subnets reported by the service.
    #[serde(rename(deserialize = "subnets", serialize = "Subnets"))]
    pub subnets: Vec<Subnet>,
}

impl Network {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("uuid"),
        field("displayName"),
        field("status"),
        field("zoneId"),
        ShapeField {
            name: "subnets",
            shape: Shape::Array(&Subnet::SCHEMA),
        },
    ]);
}

/// Database Instance returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Instance {
    /// ID reported by the service.
    #[serde(
        rename(deserialize = "id", serialize = "ID"),
        deserialize_with = "null_default"
    )]
    pub id: String,
    /// Name reported by the service.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        deserialize_with = "null_default"
    )]
    pub name: String,
    /// Status reported by the service.
    #[serde(
        rename(deserialize = "status", serialize = "Status"),
        deserialize_with = "null_default"
    )]
    pub status: String,
    /// DatastoreType reported by the service.
    #[serde(
        rename(deserialize = "datastoreType", serialize = "DatastoreType"),
        deserialize_with = "null_default"
    )]
    pub datastore_type: String,
    /// DatastoreVersion reported by the service.
    #[serde(
        rename(deserialize = "datastoreVersion", serialize = "DatastoreVersion"),
        deserialize_with = "null_default"
    )]
    pub datastore_version: String,
    /// ZoneID reported by the service.
    #[serde(
        rename(deserialize = "zoneId", serialize = "ZoneID"),
        deserialize_with = "null_default"
    )]
    pub zone_id: String,
    /// ProjectID reported by the service.
    #[serde(
        rename(deserialize = "projectId", serialize = "ProjectID"),
        deserialize_with = "null_default"
    )]
    pub project_id: String,
    /// SubnetID reported by the service.
    #[serde(
        rename(deserialize = "subnetId", serialize = "SubnetID"),
        deserialize_with = "null_default"
    )]
    pub subnet_id: String,
    /// VolumeType reported by the service.
    #[serde(
        rename(deserialize = "volumeType", serialize = "VolumeType"),
        deserialize_with = "null_default"
    )]
    pub volume_type: String,
    /// VolumeSize reported by the service.
    #[serde(
        rename(deserialize = "volumeSize", serialize = "VolumeSize"),
        deserialize_with = "null_default"
    )]
    pub volume_size: i32,
    /// QuotaPackageID reported by the service.
    #[serde(
        rename(deserialize = "quotaPackageId", serialize = "QuotaPackageID"),
        deserialize_with = "null_default"
    )]
    pub quota_package_id: String,
    /// PublicAccess reported by the service.
    #[serde(rename(deserialize = "publicAccess", serialize = "PublicAccess"))]
    pub public_access: Option<bool>,
    /// BackupAuto reported by the service.
    #[serde(rename(deserialize = "backupAuto", serialize = "BackupAuto"))]
    pub backup_auto: Option<bool>,
    /// Owner reported by the service.
    #[serde(rename(deserialize = "owner", serialize = "Owner"))]
    pub owner: Option<bool>,
    /// Role reported by the service.
    #[serde(
        rename(deserialize = "role", serialize = "Role"),
        deserialize_with = "null_default"
    )]
    pub role: String,
    /// ReplicaSourceID reported by the service.
    #[serde(
        rename(deserialize = "replicaSourceId", serialize = "ReplicaSourceID"),
        deserialize_with = "null_default"
    )]
    pub replica_source_id: String,
    /// Replicas reported by the service.
    #[serde(
        rename(deserialize = "replicas", serialize = "Replicas"),
        deserialize_with = "null_default"
    )]
    pub replicas: Vec<String>,
    /// Deleted reported by the service.
    #[serde(rename(deserialize = "deleted", serialize = "Deleted"))]
    pub deleted: Option<bool>,
}

impl Instance {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("id"),
        field("name"),
        field("status"),
        field("datastoreType"),
        field("datastoreVersion"),
        field("zoneId"),
        field("projectId"),
        field("subnetId"),
        field("volumeType"),
        field("volumeSize"),
        field("quotaPackageId"),
        field("publicAccess"),
        field("backupAuto"),
        field("owner"),
        field("role"),
        field("replicaSourceId"),
        field("replicas"),
        field("deleted"),
    ]);
}

/// Database Page returned by the service.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Page {
    /// TotalPages reported by the service.
    #[serde(
        rename(deserialize = "totalPages", serialize = "TotalPages"),
        deserialize_with = "null_default"
    )]
    pub total_pages: i32,
    /// TotalElements reported by the service.
    #[serde(
        rename(deserialize = "totalElements", serialize = "TotalElements"),
        deserialize_with = "null_default"
    )]
    pub total_elements: i32,
    /// Size reported by the service.
    #[serde(
        rename(deserialize = "size", serialize = "Size"),
        deserialize_with = "null_default"
    )]
    pub size: i32,
    /// Number reported by the service.
    #[serde(
        rename(deserialize = "number", serialize = "Number"),
        deserialize_with = "null_default"
    )]
    pub number: i32,
    /// MaxSize reported by the service.
    #[serde(
        rename(deserialize = "maxSize", serialize = "MaxSize"),
        deserialize_with = "null_default"
    )]
    pub max_size: i32,
}

impl Page {
    pub(super) const SCHEMA: Shape<'static> = Shape::Object(&[
        field("totalPages"),
        field("totalElements"),
        field("size"),
        field("number"),
        field("maxSize"),
    ]);
}
