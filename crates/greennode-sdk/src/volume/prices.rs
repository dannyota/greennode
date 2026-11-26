use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::model::pricing::{
    DiscoveryFailure, DiscoveryMetadata, DiscoveryPrice, discovery_notes, note,
};
use crate::quote::{
    catalog::{self, Catalog},
    input,
    request::{self, Product, Request, decode},
};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::wire::{failure, invalid, reflects};
use super::{Client, VolumeType, inventory::response_id};

const OP: &str = "volume.ListVolumePrices";
const QUOTE: &str = "pricing.GetQuote";

/// Selects offerings, quoted size and encryption, and discovery limits.
#[derive(Clone, Debug, Default)]
pub struct ListVolumePricesInput {
    /// Exact catalog zone. Empty includes all catalog zones.
    pub zone_id: String,
    /// Exact volume type ID. Empty includes every type.
    pub volume_type_id: String,
    /// Exact IOPS. Zero includes every offered rate.
    pub iops: i64,
    /// Quoted size in GB. Zero selects each offering's minimum.
    pub size: i64,
    /// `all`, `none`, or an exact encryption key. Empty selects `all`.
    pub encryption: String,
    /// Logical request limit. Zero selects 500, and the maximum is 2000.
    pub max_requests: i64,
    /// Return safe completed rows alongside an incomplete-discovery error.
    pub allow_partial: bool,
}

/// Available volume offerings with their quoted prices and coverage.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListVolumePricesOutput {
    /// Rows in zone, type name, type ID, and encryption order.
    pub items: Vec<VolumePrice>,
    /// Discovery coverage and caveats.
    pub metadata: DiscoveryMetadata,
}

/// One volume offering and the price quoted for the requested size.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumePrice {
    /// Volume type and supported size bounds.
    pub volume_type: VolumeType,
    /// Zone identifier.
    #[serde(rename = "ZoneID")]
    pub zone_id: String,
    /// Zone display name.
    pub zone_name: String,
    /// Encryption key. Empty represents an unencrypted volume.
    #[serde(rename = "EncryptionTypeID")]
    pub encryption_type_id: String,
    /// Whether the catalog marked the zone enabled.
    pub zone_enabled: Option<bool>,
    /// Size used for the quote.
    #[serde(rename = "SizeGB")]
    pub size_gb: i64,
    /// Effective monthly price per provisioned GB.
    #[serde(rename = "PricePerGB")]
    pub price_per_gb: Option<f64>,
    /// Monthly quote and status.
    pub price: DiscoveryPrice,
}

impl VolumePrice {
    fn type_id(&self) -> &str {
        if self.volume_type.volume_type_id.is_empty() {
            &self.volume_type.id
        } else {
            &self.volume_type.volume_type_id
        }
    }

    fn order(&self) -> (&str, &str, &str, &str) {
        (
            &self.zone_id,
            &self.volume_type.name,
            self.type_id(),
            &self.encryption_type_id,
        )
    }

    fn item_key(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.zone_id,
            self.volume_type.volume_type_zone_id,
            self.type_id(),
            self.encryption_type_id
        )
    }
}

struct Discovery<'a> {
    catalog: Catalog<'a>,
    out: ListVolumePricesOutput,
    errors: Vec<Error>,
    allow_partial: bool,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Discovers monthly prices at the requested size or each offering's minimum.
    pub async fn list_volume_prices(
        &self,
        ctx: &RequestContext,
        input: ListVolumePricesInput,
    ) -> Result<ListVolumePricesOutput, Failure<ListVolumePricesOutput>> {
        validate(&input).map_err(failure)?;
        let mut notes = discovery_notes();
        notes.retain(|note| {
            !matches!(
                note.scope.as_str(),
                "compute" | "network" | "object-storage"
            )
        });
        notes.push(note(
            "exclusions",
            "volume",
            "Provisioned volume quotes exclude snapshots, backups, and server CES.",
            "",
            "observed",
        ));
        let mut run = Discovery {
            catalog: Catalog::new(&self.session, ctx, input.max_requests),
            out: ListVolumePricesOutput {
                items: Vec::new(),
                metadata: DiscoveryMetadata {
                    region: self.region.clone(),
                    complete: true,
                    requests: 0,
                    quoted_at: self.session.clock().utc_now(),
                    failures: Vec::new(),
                    notes,
                    excluded_groups: None,
                },
            },
            errors: Vec::new(),
            allow_partial: input.allow_partial,
        };
        run.discover(&input).await.map_err(failure)?;
        run.finish()
    }
}

fn validate(input: &ListVolumePricesInput) -> Result<(), Box<Error>> {
    if input.size < 0 || input.iops < 0 {
        return Err(invalid(
            "vngcloud: invalid input: Size and IOPS must be nonnegative",
        ));
    }
    for (name, value) in [
        ("ZoneID", &input.zone_id),
        ("VolumeTypeID", &input.volume_type_id),
        ("Encryption", &input.encryption),
    ] {
        if !value.is_empty() && !response_id(value) {
            return Err(invalid(format!(
                "vngcloud: invalid input: {OP} requires {name} to match ^[A-Za-z0-9_-]+$"
            )));
        }
    }
    if !(0..=2000).contains(&input.max_requests) {
        return Err(invalid(
            "vngcloud: invalid input: discovery limit must be between 1 and 2000",
        ));
    }
    Ok(())
}

impl Discovery<'_> {
    async fn discover(&mut self, input: &ListVolumePricesInput) -> Result<(), Box<Error>> {
        let project = self.catalog.project().await?;
        let zones = self.catalog.zones(&project).await?;
        if !input.zone_id.is_empty() && !zones.iter().any(|zone| zone.id == input.zone_id) {
            return Err(invalid("vngcloud: invalid input: unknown ZoneID"));
        }
        let parents = self.catalog.volume_parents(&project).await?;
        let encryptions = self.catalog.encryptions(&project).await?;
        let all_encryption = matches!(input.encryption.as_str(), "" | "all");
        let mut choices = Vec::new();
        if all_encryption || input.encryption == "none" {
            choices.push("");
        }
        choices.extend(
            encryptions
                .iter()
                .filter(|row| all_encryption || input.encryption == row.key)
                .map(|row| row.key.as_str()),
        );
        for parent in parents {
            let types = match self.catalog.volume_types(&project, &parent.id, true).await {
                Ok(types) => types,
                Err(error) => {
                    if self.record(catalog::TYPES, &parent.id, error)? {
                        return Ok(());
                    }
                    continue;
                }
            };
            for volume_type in types {
                if (!input.volume_type_id.is_empty()
                    && input.volume_type_id != volume_type.quote_id())
                    || (input.iops > 0 && input.iops != volume_type.iops)
                {
                    continue;
                }
                let size = if input.size == 0 {
                    volume_type.min_size
                } else {
                    input.size
                };
                for zone in zones
                    .iter()
                    .filter(|zone| input.zone_id.is_empty() || input.zone_id == zone.id)
                {
                    let disabled = input.zone_id.is_empty() && zone.enabled == Some(false);
                    let outside = !(volume_type.min_size..=volume_type.max_size).contains(&size);
                    for encryption in &choices {
                        if let Err(error) = self.catalog.rows(1) {
                            self.record(OP, "rows", error)?;
                            return Ok(());
                        }
                        let status = if disabled || outside {
                            "inapplicable"
                        } else {
                            "unknown"
                        };
                        if let Some(reason) = inapplicable(disabled, outside) {
                            self.out.metadata.notes.push(note(
                                "inapplicable",
                                &format!("{}/{}", zone.id, volume_type.quote_id()),
                                reason,
                                "",
                                "observed",
                            ));
                        }
                        let kind = VolumeType {
                            id: volume_type.id.clone(),
                            volume_type_id: volume_type.volume_type_id.clone(),
                            name: volume_type.name.clone(),
                            iops: volume_type.iops,
                            max_size: volume_type.max_size,
                            min_size: volume_type.min_size,
                            throughput: volume_type.throughput,
                            zone_id: volume_type.zone_id.clone(),
                            volume_type_zone_id: parent.id.clone(),
                        };
                        let row = VolumePrice {
                            volume_type: kind,
                            zone_id: zone.id.clone(),
                            zone_name: zone.name.clone(),
                            encryption_type_id: (*encryption).into(),
                            zone_enabled: zone.enabled,
                            size_gb: size,
                            price_per_gb: None,
                            price: DiscoveryPrice {
                                status: status.into(),
                                source: QUOTE.into(),
                                ..Default::default()
                            },
                        };
                        if serde_json::to_value(&row)
                            .map_or(true, |value| reflects(&self.catalog.evidence, &value))
                        {
                            if self.record(OP, "rows", request::invalid(OP, 200))? {
                                return Ok(());
                            }
                            continue;
                        }
                        self.out.items.push(row);
                    }
                }
            }
        }
        self.quote().await
    }

    async fn quote(&mut self) -> Result<(), Box<Error>> {
        let mut group_indexes = HashMap::new();
        let mut groups = Vec::<(Value, Vec<usize>)>::new();
        for (index, item) in self
            .out
            .items
            .iter()
            .enumerate()
            .filter(|(_, row)| row.price.status == "unknown")
        {
            let body = input::volume(
                &item.zone_id,
                item.type_id(),
                item.size_gb,
                &item.encryption_type_id,
            );
            let next = groups.len();
            let group = *group_indexes.entry(body.to_string()).or_insert(next);
            if group == next {
                groups.push((body, Vec::new()));
            }
            if let Some((_, indexes)) = groups.get_mut(group) {
                indexes.push(index);
            }
        }
        if let Err(error) = self.catalog.preflight(groups.len()) {
            self.record(OP, "quotes", error)?;
            return Ok(());
        }
        for (body, indexes) in groups {
            let result = self.quote_group(&body).await;
            let mut stop = false;
            for index in indexes {
                let Some(row) = self.out.items.get_mut(index) else {
                    continue;
                };
                let rate = result
                    .as_ref()
                    .ok()
                    .and_then(|price| price.monthly_price)
                    .map(|amount| amount / row.size_gb as f64);
                if let Ok(price) = &result
                    && !rate.is_some_and(|rate| {
                        self.catalog.evidence.reflects_fixed_two(rate)
                            || reflects(&self.catalog.evidence, &serde_json::json!(rate))
                    })
                {
                    row.price.clone_from(price);
                    row.price_per_gb = rate;
                } else {
                    row.price.status = "failed".into();
                    let key = row.item_key();
                    let error = result.as_ref().err().map_or_else(
                        || request::invalid(QUOTE, 200),
                        |error| Box::new(copy_error(error)),
                    );
                    stop |= self.record(QUOTE, &key, error)?;
                }
            }
            if stop {
                break;
            }
        }
        Ok(())
    }

    async fn quote_group(&mut self, body: &Value) -> Result<DiscoveryPrice, Box<Error>> {
        if reflects(&self.catalog.evidence, body) {
            return Err(invalid(
                "vngcloud: invalid input: request contains credential material",
            ));
        }
        let raw = self
            .catalog
            .exchange(Request {
                operation: QUOTE,
                template: "v1/price",
                product: Product::Portal,
                path: &[],
                query: &[],
                body: Some(body),
            })
            .await?;
        let data = catalog::value(QUOTE, &raw)?;
        let price = decode::monthly(QUOTE, &data)?;
        if serde_json::to_value(&price)
            .map_or(true, |value| reflects(&self.catalog.evidence, &value))
            || price
                .monthly_price
                .is_some_and(|amount| self.catalog.evidence.reflects_fixed_two(amount))
        {
            return Err(request::invalid(QUOTE, 200));
        }
        self.catalog.rows(0)?;
        Ok(price)
    }

    fn record(
        &mut self,
        operation: &'static str,
        key: &str,
        error: Box<Error>,
    ) -> Result<bool, Box<Error>> {
        if !self.allow_partial {
            return Err(error);
        }
        let stop = [
            ErrorClass::Auth,
            ErrorClass::RateLimited,
            ErrorClass::Cancelled,
            ErrorClass::DeadlineExceeded,
            ErrorClass::EstimateLimit,
        ]
        .into_iter()
        .any(|class| error.has_class(class))
            || error
                .api_error()
                .is_some_and(|api| matches!(api.status(), Some(401 | 429)));
        let status = error.api_error().and_then(crate::ApiError::status);
        let code = if error
            .api_error()
            .is_some_and(|api| api.code() == "InvalidResponse")
        {
            "InvalidResponse"
        } else {
            resolved_code(status, error.api_error().is_some())
        };
        self.out.metadata.failures.push(DiscoveryFailure {
            operation: operation.into(),
            item_key: safe_text(key, &self.catalog.evidence),
            code: code.into(),
            status: i64::from(status.unwrap_or_default()),
        });
        self.out.metadata.complete = false;
        self.errors.push(*error);
        Ok(stop)
    }

    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Failure by value"
    )]
    fn finish(mut self) -> Result<ListVolumePricesOutput, Failure<ListVolumePricesOutput>> {
        self.out
            .items
            .sort_by(|left, right| left.order().cmp(&right.order()));
        self.out.metadata.requests = i64::try_from(self.catalog.requests()).unwrap_or_default();
        self.out.metadata.quoted_at = self.catalog.session.clock().utc_now();
        for note in &mut self.out.metadata.notes {
            note.scope = safe_text(&note.scope, &self.catalog.evidence);
        }
        for item in &mut self.out.metadata.failures {
            item.item_key = safe_text(&item.item_key, &self.catalog.evidence);
        }
        let safe = serde_json::to_value(&self.out)
            .is_ok_and(|value| !reflects(&self.catalog.evidence, &value));
        if !safe {
            return Err(failure(request::invalid(OP, 200)));
        }
        if self.errors.is_empty() {
            return Ok(self.out);
        }
        if self.out.items.is_empty() {
            return Err(failure(Box::new(self.errors.remove(0))));
        }
        let causes = self
            .errors
            .into_iter()
            .rev()
            .reduce(|tail, error| error.with_cause(tail));
        let mut outcome = Error::new(ErrorClass::DiscoveryIncomplete);
        if let Some(causes) = causes {
            outcome = outcome.with_cause(causes);
        }
        Err(Failure::new(
            outcome,
            Some(self.out),
            WriteDispatch::NotAWrite,
        ))
    }
}

fn resolved_code(status: Option<u16>, api: bool) -> &'static str {
    match status {
        Some(400) => "BadRequest",
        Some(401) => "Unauthorized",
        Some(403) => "Forbidden",
        Some(404) => "NotFound",
        Some(409) => "Conflict",
        Some(429) => "Throttled",
        Some(500..) => "ServerError",
        Some(400..) => "ClientError",
        _ if api => "",
        _ => "DiscoveryIncomplete",
    }
}

fn copy_error(error: &Error) -> Error {
    error.api_error().map_or_else(
        || Error::new(error.class()),
        |api| {
            let cause = request::api(
                api.operation(),
                api.status(),
                api.code(),
                api.message(),
                api.retryable(),
            );
            if error.class() == cause.class() {
                cause
            } else {
                Error::new(error.class()).with_cause(cause)
            }
        },
    )
}

fn safe_text(text: &str, evidence: &crate::wire::product::evidence::Evidence) -> String {
    if evidence.reflects_encoded_text(text) {
        "<redacted>".into()
    } else {
        text.into()
    }
}

fn inapplicable(disabled: bool, outside: bool) -> Option<&'static str> {
    match (disabled, outside) {
        (true, true) => {
            Some("The zone is disabled and the requested size is outside the offering's bounds.")
        }
        (true, false) => Some("The zone is disabled."),
        (false, true) => Some("The requested size is outside the offering's bounds."),
        (false, false) => None,
    }
}
