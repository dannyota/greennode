use serde::Serialize;

use crate::model::pricing::{DiscoveryMetadata, DiscoveryPrice};

use super::Flavor;

mod catalog;
mod run;

/// Filters and limits for flavor price discovery.
#[derive(Clone, Debug, Default)]
pub struct ListFlavorPricesInput {
    /// Selects one returned zone; empty selects all zones in the configured region.
    pub zone_id: String,
    /// Selects a returned root volume type; empty selects a 3000 IOPS type.
    pub root_disk_type_id: String,
    /// Filters by exact, case-sensitive family key when nonempty.
    pub family: String,
    /// Filters by exact, case-sensitive platform key when nonempty.
    pub platform: String,
    /// Filters by exact, case-sensitive flavor name when nonempty.
    pub name: String,
    /// Is the lower raw memory bound; zero leaves it unbounded.
    pub min_ram: i64,
    /// Is the upper raw memory bound; zero leaves it unbounded.
    pub max_ram: i64,
    /// Includes non-public catalogs when set.
    pub include_non_public: bool,
    /// Excludes sold-out flavors and flavors without remaining VMs.
    pub available_only: bool,
    /// Limits physical requests; zero selects 500 and the maximum is 2000.
    pub max_requests: i64,
    /// Retains partial results and reports incomplete discovery when set.
    pub allow_partial: bool,
}

/// Flavor price rows and discovery metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListFlavorPricesOutput {
    /// Items.
    #[serde(rename = "Items")]
    pub items: Vec<FlavorPrice>,
    /// Metadata.
    #[serde(rename = "Metadata")]
    pub metadata: DiscoveryMetadata,
}

/// One catalog flavor and the price of its reference instance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlavorPrice {
    /// Flavor.
    #[serde(rename = "Flavor")]
    pub flavor: Flavor,
    /// Zone id.
    #[serde(rename = "ZoneID")]
    pub zone_id: String,
    /// Zone name.
    #[serde(rename = "ZoneName")]
    pub zone_name: String,
    /// Family.
    #[serde(rename = "Family")]
    pub family: String,
    /// Family name.
    #[serde(rename = "FamilyName")]
    pub family_name: String,
    /// Platform.
    #[serde(rename = "Platform")]
    pub platform: String,
    /// Visibility.
    #[serde(rename = "Visibility")]
    pub visibility: String,
    /// Ram unit.
    #[serde(rename = "RAMUnit")]
    pub ram_unit: String,
    /// Zone enabled.
    #[serde(rename = "ZoneEnabled")]
    pub zone_enabled: Option<bool>,
    /// Ram gi b.
    #[serde(rename = "RAMGiB")]
    pub ram_gib: Option<f64>,
    /// Price per gi b.
    #[serde(rename = "PricePerGiB")]
    pub price_per_gib: Option<f64>,
    /// Price per ram unit.
    #[serde(rename = "PricePerRAMUnit")]
    pub price_per_ram_unit: Option<f64>,
    /// Reference.
    #[serde(rename = "Reference")]
    pub reference: Option<FlavorPriceReference>,
    /// Price.
    #[serde(rename = "Price")]
    pub price: DiscoveryPrice,
}

/// The image and root disk chosen for an instance quote.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlavorPriceReference {
    /// Image id.
    #[serde(rename = "ImageID")]
    pub image_id: String,
    /// Image type.
    #[serde(rename = "ImageType")]
    pub image_type: String,
    /// Image version.
    #[serde(rename = "ImageVersion")]
    pub image_version: String,
    /// Root disk type id.
    #[serde(rename = "RootDiskTypeID")]
    pub root_disk_type_id: String,
    /// Root disk size gb.
    #[serde(rename = "RootDiskSizeGB")]
    pub root_disk_size_gb: i64,
}
