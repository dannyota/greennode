use serde::{Serialize, Serializer};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

/// Quote amount and basis, with unknown amounts left absent.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DiscoveryPrice {
    /// Pricing status.
    #[serde(rename = "Status")]
    pub status: String,
    /// Basis of the price.
    #[serde(rename = "Basis")]
    pub basis: String,
    /// Quote currency.
    #[serde(rename = "Currency")]
    pub currency: String,
    /// Quote tax treatment.
    #[serde(rename = "Tax")]
    pub tax: String,
    /// Operation or published source of the price.
    #[serde(rename = "Source")]
    pub source: String,
    /// Monthly amount when known.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: Option<f64>,
    /// Original price when returned.
    #[serde(rename = "OriginalPrice")]
    pub original_price: Option<f64>,
    /// Discount amount when returned.
    #[serde(rename = "DiscountPrice")]
    pub discount_price: Option<f64>,
    /// Discount percentage when returned.
    #[serde(rename = "DiscountPercent")]
    pub discount_percent: Option<f64>,
}

/// Coverage and completion metadata for one discovery invocation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiscoveryMetadata {
    /// Configured region.
    #[serde(rename = "Region")]
    pub region: String,
    /// Whether processing finished without failures.
    #[serde(rename = "Complete")]
    pub complete: bool,
    /// Number of logical requests attempted.
    #[serde(rename = "Requests")]
    pub requests: i64,
    #[serde(serialize_with = "quoted_at")]
    /// UTC completion time.
    #[serde(rename = "QuotedAt")]
    pub quoted_at: time::OffsetDateTime,
    /// Safe details of failed discovery operations.
    #[serde(rename = "Failures")]
    pub failures: Vec<DiscoveryFailure>,
    /// Pricing context and caveats.
    #[serde(rename = "Notes")]
    pub notes: Vec<PricingNote>,
    /// Number of excluded groups when reported.
    #[serde(rename = "ExcludedGroups")]
    pub excluded_groups: Option<i64>,
}

/// Safe scope and classification of a discovery failure.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DiscoveryFailure {
    /// Operation associated with the failure.
    #[serde(rename = "Operation")]
    pub operation: String,
    /// Affected component key when applicable.
    #[serde(rename = "ItemKey")]
    pub item_key: String,
    /// Safe classification code.
    #[serde(rename = "Code")]
    pub code: String,
    /// HTTP status, zero when unavailable.
    #[serde(rename = "Status")]
    pub status: i64,
}

/// Published or observed pricing context.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PricingNote {
    /// Note topic.
    #[serde(rename = "Topic")]
    pub topic: String,
    /// Scope of the note.
    #[serde(rename = "Scope")]
    pub scope: String,
    /// Note text.
    #[serde(rename = "Text")]
    pub text: String,
    /// First-party source URL when available.
    #[serde(rename = "SourceURL")]
    pub source_url: String,
    /// Evidence class.
    #[serde(rename = "Evidence")]
    pub evidence: String,
}

fn quoted_at<S: Serializer>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    let text = value
        .to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&text)
}

pub(crate) fn note(
    topic: &str,
    scope: &str,
    text: &str,
    source_url: &str,
    evidence: &str,
) -> PricingNote {
    PricingNote {
        topic: topic.into(),
        scope: scope.into(),
        text: text.into(),
        source_url: source_url.into(),
        evidence: evidence.into(),
    }
}

/// Returns the reviewed pricing context for discovery results.
pub fn discovery_notes() -> Vec<PricingNote> {
    vec![
        note(
            "account-scope",
            "quotes",
            "Quotes reflect the current account. Returned discounts do not establish eligibility for a program.",
            "https://docs.greennode.ai/partner-portal-user-guide/registration-of-partner-discount.md",
            "published",
        ),
        note(
            "terms",
            "monthly",
            "Tested monthly quotes showed no term discount on the observed account. This is not a universal discount policy or proof that a term was honored.",
            "",
            "observed",
        ),
        note(
            "units",
            "compute",
            "The byte definition of raw RAM is unverified. GiB amounts and rates remain unknown.",
            "https://docs.greennode.ai/vserver/compute-hcm03-1a/instance/flavor.md",
            "unverified",
        ),
        note(
            "exclusions",
            "compute",
            "Instance prices exclude root and data disks, CES, licensing, IPs, bandwidth, traffic, snapshots, backups, and NAT.",
            "https://docs.greennode.ai/vserver/compute-hcm03-1a/pricing.md",
            "published",
        ),
        note(
            "effective-rate",
            "storage",
            "Price per GB is the effective rate at the quoted size, not a marginal tariff.",
            "https://docs.greennode.ai/vstorage/object-storage/cach-tinh-phi.md",
            "published",
        ),
        note(
            "egress",
            "object-storage",
            "Published egress rules conflict. Egress and request charges remain unknown and are excluded from quota quotes.",
            "https://docs.greennode.ai/vstorage/object-storage/cach-tinh-phi.md",
            "conflicting",
        ),
        note(
            "bandwidth",
            "network",
            "Bandwidth tariffs and capacity units are unverified and excluded from public-IP prices.",
            "https://docs.greennode.ai/vserver/compute-hcm03-1a/vpc/bandwidth.md",
            "unverified",
        ),
    ]
}
