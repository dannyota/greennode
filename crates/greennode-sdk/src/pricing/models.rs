use crate::model::pricing::{DiscoveryMetadata, DiscoveryPrice};
use serde::{Deserialize, Serialize};

fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Option::deserialize(d)?.unwrap_or_default())
}

fn nullable_rows<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<Option<T>>>::deserialize(d)?
        .unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

/// Setup to price using independent monthly unit quotes.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimateInput {
    #[serde(deserialize_with = "nullable_rows")]
    /// Server groups including root and optional data disks.
    #[serde(rename = "Servers")]
    pub servers: Vec<EstimateServer>,
    #[serde(deserialize_with = "nullable_rows")]
    /// Independent additional disk groups.
    #[serde(rename = "Volumes")]
    pub volumes: Vec<EstimateVolume>,
    #[serde(deserialize_with = "nullable_rows")]
    /// Public IP product groups.
    #[serde(rename = "PublicIPs")]
    pub public_ips: Vec<EstimatePublicIp>,
    #[serde(deserialize_with = "nullable_rows")]
    /// Independent monthly storage packages.
    #[serde(rename = "Storage")]
    pub storage: Vec<EstimateStorage>,
    #[serde(deserialize_with = "nullable")]
    /// Maximum logical requests. Zero selects 500.
    #[serde(rename = "MaxRequests")]
    pub max_requests: i64,
    #[serde(deserialize_with = "nullable")]
    /// Preserve surviving results after an operational or arithmetic failure.
    #[serde(rename = "AllowPartial")]
    pub allow_partial: bool,
}

/// Identical server units with root and optional bundled data disks.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimateServer {
    #[serde(deserialize_with = "nullable")]
    /// Unique component key.
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(deserialize_with = "nullable")]
    /// Explicit zone identifier from the catalog.
    #[serde(rename = "ZoneID")]
    pub zone_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Flavor identifier sent to the quote gateway.
    #[serde(rename = "FlavorID")]
    pub flavor_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Image identifier from the zone catalog.
    #[serde(rename = "ImageID")]
    pub image_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Number of identical units from 1 to 1000.
    #[serde(rename = "Quantity")]
    pub quantity: i64,
    #[serde(deserialize_with = "nullable")]
    /// Root disk size in GB per unit.
    #[serde(rename = "RootDiskSize")]
    pub root_disk_size: i64,
    #[serde(deserialize_with = "nullable")]
    /// Root disk type identifier.
    #[serde(rename = "RootDiskTypeID")]
    pub root_disk_type_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Optional root disk encryption type identifier.
    #[serde(rename = "RootDiskEncryptionTypeID")]
    pub root_disk_encryption_type_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Data disk size in GB per unit. Zero omits the disk.
    #[serde(rename = "DataDiskSize")]
    pub data_disk_size: i64,
    #[serde(deserialize_with = "nullable")]
    /// Bundled data disk type identifier when a disk is included.
    #[serde(rename = "DataDiskTypeID")]
    pub data_disk_type_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Optional data disk encryption type identifier.
    #[serde(rename = "DataDiskEncryptionTypeID")]
    pub data_disk_encryption_type_id: String,
}

/// Identical additional disks, counted separately from servers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimateVolume {
    #[serde(deserialize_with = "nullable")]
    /// Unique component key.
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(deserialize_with = "nullable")]
    /// Optional key of the associated server group in the same zone.
    #[serde(rename = "ServerKey")]
    pub server_key: String,
    #[serde(deserialize_with = "nullable")]
    /// Explicit zone identifier from the catalog.
    #[serde(rename = "ZoneID")]
    pub zone_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Disk type identifier.
    #[serde(rename = "VolumeTypeID")]
    pub volume_type_id: String,
    #[serde(deserialize_with = "nullable")]
    /// Number of identical units from 1 to 1000.
    #[serde(rename = "Quantity")]
    pub quantity: i64,
    #[serde(deserialize_with = "nullable")]
    /// Disk size in GB per unit.
    #[serde(rename = "Size")]
    pub size: i64,
    #[serde(deserialize_with = "nullable")]
    /// Optional disk encryption type identifier.
    #[serde(rename = "EncryptionTypeID")]
    pub encryption_type_id: String,
}

/// Independent public IP products.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimatePublicIp {
    #[serde(deserialize_with = "nullable")]
    /// Unique component key.
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(deserialize_with = "nullable")]
    /// Public IP product kind.
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(deserialize_with = "nullable")]
    /// Explicit public VIP type, empty for other kinds.
    #[serde(rename = "Type")]
    pub r#type: String,
    #[serde(deserialize_with = "nullable")]
    /// Number of identical units from 1 to 1000.
    #[serde(rename = "Quantity")]
    pub quantity: i64,
}

/// Independent monthly storage packages.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimateStorage {
    #[serde(deserialize_with = "nullable")]
    /// Unique component key.
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(deserialize_with = "nullable")]
    /// Storage catalog region name or ID. Empty uses the configured region.
    #[serde(rename = "Region")]
    pub region: String,
    #[serde(deserialize_with = "nullable")]
    /// Exact active project type with a monthly offering.
    #[serde(rename = "Type")]
    pub r#type: String,
    #[serde(deserialize_with = "nullable")]
    /// Number of identical packages from 1 to 1000.
    #[serde(rename = "Quantity")]
    pub quantity: i64,
    #[serde(deserialize_with = "nullable")]
    /// Quota in GB per package.
    #[serde(rename = "QuotaGB")]
    pub quota_gb: i64,
}

/// Ordered quote rows, VND totals, coverage and processing metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EstimateOutput {
    /// Requested rows in setup order.
    #[serde(rename = "Items")]
    pub items: Vec<EstimateItem>,
    /// Independent monthly totals for specified components.
    #[serde(rename = "Total")]
    pub total: EstimateTotal,
    /// Quoted, unknown, excluded and failed coverage.
    #[serde(rename = "Coverage")]
    pub coverage: Vec<EstimateCoverage>,
    /// Request counts and completion information.
    #[serde(rename = "Metadata")]
    pub metadata: DiscoveryMetadata,
}

/// One requested component group.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EstimateItem {
    /// Unique component key.
    #[serde(rename = "Key")]
    pub key: String,
    /// Resource kind.
    #[serde(rename = "Kind")]
    pub kind: String,
    /// Associated server group if specified.
    #[serde(rename = "ServerKey")]
    pub server_key: String,
    /// Regional endpoint or resolved storage region.
    #[serde(rename = "Region")]
    pub region: String,
    /// Explicit zone if applicable.
    #[serde(rename = "ZoneID")]
    pub zone_id: String,
    /// Number of units in the group.
    #[serde(rename = "Quantity")]
    pub quantity: i64,
    /// Price for one unit.
    #[serde(rename = "Price")]
    pub price: DiscoveryPrice,
    /// Price multiplied by quantity, or absent if unknown.
    #[serde(rename = "MonthlyAmount")]
    pub monthly_amount: Option<f64>,
    /// Informational charges included in the unit price.
    #[serde(rename = "Breakdown")]
    pub breakdown: Vec<EstimateCharge>,
}

/// One informational charge per unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EstimateCharge {
    /// Charge name from the quote.
    #[serde(rename = "Name")]
    pub name: String,
    /// Monthly charge if supplied.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: Option<f64>,
}

/// Independent monthly VND quote totals and completeness.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EstimateTotal {
    /// Sum of valid monthly amounts when finite and known.
    #[serde(rename = "KnownMonthlySubtotal")]
    pub known_monthly_subtotal: Option<f64>,
    /// Sum when every requested row was quoted.
    #[serde(rename = "RequestedMonthlyTotal")]
    pub requested_monthly_total: Option<f64>,
    /// Currency of the amounts.
    #[serde(rename = "Currency")]
    pub currency: String,
    /// Tax basis of the amounts.
    #[serde(rename = "Tax")]
    pub tax: String,
    /// Accounting basis for the totals.
    #[serde(rename = "Basis")]
    pub basis: String,
    /// Scope of the estimate.
    #[serde(rename = "Scope")]
    pub scope: String,
    /// Whether every specified row was successfully priced.
    #[serde(rename = "RequestedComplete")]
    pub requested_complete: bool,
    /// Whether the estimate covers the entire bill. Always false.
    #[serde(rename = "CostComplete")]
    pub cost_complete: bool,
}

/// Coverage for a requested component or the entire setup.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EstimateCoverage {
    /// Coverage topic.
    #[serde(rename = "Topic")]
    pub topic: String,
    /// Component key or setup scope.
    #[serde(rename = "Scope")]
    pub scope: String,
    /// Coverage status.
    #[serde(rename = "Status")]
    pub status: String,
    /// Reason for the status.
    #[serde(rename = "Reason")]
    pub reason: String,
}

impl EstimateInput {
    /// Checks setup fields independent of the configured region or catalogs.
    pub fn validate(&self) -> Result<(), &'static str> {
        super::validate::setup(self, "han-1")
    }
}

impl EstimateServer {
    /// Creates a server group with the required selectors, count and root disk.
    pub fn new(
        key: impl Into<String>,
        zone_id: impl Into<String>,
        flavor_id: impl Into<String>,
        image_id: impl Into<String>,
        quantity: i64,
        root_disk_size: i64,
        root_disk_type_id: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            zone_id: zone_id.into(),
            flavor_id: flavor_id.into(),
            image_id: image_id.into(),
            quantity,
            root_disk_size,
            root_disk_type_id: root_disk_type_id.into(),
            ..Self::default()
        }
    }
}

impl EstimateVolume {
    /// Creates an independent volume group with the required selectors, count and size.
    pub fn new(
        key: impl Into<String>,
        zone_id: impl Into<String>,
        volume_type_id: impl Into<String>,
        quantity: i64,
        size: i64,
    ) -> Self {
        Self {
            key: key.into(),
            zone_id: zone_id.into(),
            volume_type_id: volume_type_id.into(),
            quantity,
            size,
            ..Self::default()
        }
    }
}

impl EstimatePublicIp {
    /// Creates an IP group with the given kind, optional VIP type and count.
    pub fn new(
        key: impl Into<String>,
        kind: impl Into<String>,
        product_type: impl Into<String>,
        quantity: i64,
    ) -> Self {
        Self {
            key: key.into(),
            kind: kind.into(),
            r#type: product_type.into(),
            quantity,
        }
    }
}

impl EstimateStorage {
    /// Creates a storage group in the configured region.
    pub fn new(
        key: impl Into<String>,
        project_type: impl Into<String>,
        quantity: i64,
        quota_gb: i64,
    ) -> Self {
        Self {
            key: key.into(),
            r#type: project_type.into(),
            quantity,
            quota_gb,
            ..Self::default()
        }
    }
}

/// Identifies a local estimate arithmetic overflow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EstimateArithmeticError {
    /// Multiplication or subtotal stage.
    pub stage: &'static str,
    /// Component key when multiplication failed.
    pub item_key: String,
}
