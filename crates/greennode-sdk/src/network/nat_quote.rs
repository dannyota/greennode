use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};

use crate::model::wire::{Field, go_json, model};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext};

use super::Client;
use super::inventory::screen;
use super::nat::failure;
use super::nat_catalog::{NatScope, catalog_data, catalog_invalid, decode_rows, params, reflects};
use super::wire::{Fields, Read, Style};

pub(super) const OP: &str = "network.QuoteCreateNATInstance";
const PRICE: Read = Read::new(OP, "vnetwork/billing/v1/{}/{}/price", Style::NatCatalog);
const WHITELIST: Read = PRICE.at("vnetwork/v1/{}/{}/nats/v3-whitelist", Style::NatCatalog);
const VPCS: Read = PRICE.at("vnetwork/v1/{}/{}/nats/vpcs", Style::NatCatalog);

/// Selects the Public NAT package and placement to quote.
#[derive(Clone)]
pub struct QuoteCreateNatInstanceInput {
    /// Proposed NAT display name.
    pub name: String,
    /// Regional zone identifier.
    pub zone_id: String,
    /// Availability-zone identifier from the NAT catalog.
    pub availability_zone_id: String,
    /// VND package identifier from the NAT catalog.
    pub package_id: String,
    /// VPC selected for NAT placement.
    pub vpc_id: String,
    /// Optional numeric maximum from the shared creation input. Quotes do not enforce it.
    pub max_price: f64,
}

crate::error::debug_record!(QuoteCreateNatInstanceInput {
    name,
    zone_id,
    availability_zone_id,
    package_id,
    vpc_id,
    max_price
});

impl QuoteCreateNatInstanceInput {
    /// Selects a name, region zone, availability zone, package, and VPC.
    pub fn new(
        name: impl Into<String>,
        zone_id: impl Into<String>,
        availability_zone_id: impl Into<String>,
        package_id: impl Into<String>,
        vpc_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            zone_id: zone_id.into(),
            availability_zone_id: availability_zone_id.into(),
            package_id: package_id.into(),
            vpc_id: vpc_id.into(),
            max_price: 0.0,
        }
    }
}

/// Monthly Public NAT quote in VND.
#[derive(Clone, PartialEq, Serialize)]
pub struct NetworkQuoteOutput {
    /// Quoted optimum price.
    #[serde(rename = "OptimumPrice")]
    pub optimum_price: f64,
    /// Original price.
    #[serde(rename = "OriginalPrice")]
    pub original_price: f64,
    /// Discount amount.
    #[serde(rename = "DiscountPrice")]
    pub discount_price: f64,
    /// Optional discount percentage.
    #[serde(rename = "DiscountPercent")]
    pub discount_percent: Option<f64>,
    /// Service line items in provider order.
    #[serde(rename = "Properties")]
    pub properties: Vec<NetworkPriceProperty>,
    /// Monthly price.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: f64,
    /// Total price for the quoted period.
    #[serde(rename = "TotalPrice")]
    pub total_price: f64,
    /// Quote currency.
    #[serde(rename = "Currency")]
    pub currency: String,
}

crate::wire::product::output::public_fields!(NetworkQuoteOutput {
    optimum_price,
    original_price,
    discount_price,
    discount_percent,
    properties,
    monthly_price,
    total_price,
    currency
});

/// Result of quoting Public NAT placement.
pub type QuoteCreateNatInstanceOutput = NetworkQuoteOutput;

model! {
    /// Line item from the Public NAT quote.
    pub struct NetworkPriceProperty {
        /// Quoted optimum price.
        optimum_price: f64 => "optimumPrice", "OptimumPrice";
        /// Monthly price.
        monthly_price: f64 => "monthlyPrice", "MonthlyPrice";
        /// Current price when available.
        current_price: Option<f64> => "currentPrice", "CurrentPrice";
        /// Discount percentage when available.
        discount_percent: Option<f64> => "discountPercent", "DiscountPercent";
        /// Item name.
        name: String => "name", "Name";
        /// Item description.
        description: String => "description", "Description";
    }
}

#[derive(Deserialize)]
struct PickerVpc {
    uuid: String,
    #[serde(rename = "projectUuid")]
    project: String,
    #[serde(rename = "regionId")]
    region: String,
    #[serde(rename = "regionUuid")]
    region_uuid: Option<String>,
    #[serde(default)]
    zones: Option<Vec<PickerZone>>,
}

#[derive(Deserialize)]
struct PickerZone {
    uuid: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PageInfo {
    size: i64,
    pages: i64,
    total: i64,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Checks catalog and placement, then requests a quote without placing an order.
    pub async fn quote_create_nat_instance(
        &self,
        ctx: &RequestContext,
        input: QuoteCreateNatInstanceInput,
    ) -> Result<QuoteCreateNatInstanceOutput, Failure<QuoteCreateNatInstanceOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            check_input(&input, &evidence).map_err(failure)?;
            let scope = self
                .nat_scope(ctx, PRICE, input.zone_id.clone(), &mut evidence, false)
                .await
                .map_err(|error| failure(screen(error, &evidence)))?;
            self.check_nat_placement(ctx, &scope, &input, OP, &mut evidence)
                .await
                .map_err(|error| failure(screen(error, &evidence)))?;
            self.nat_purchase_price(ctx, &scope, &input, OP, &mut evidence)
                .await
                .map_err(failure)
        })
    }

    pub(super) async fn nat_purchase_price(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        input: &QuoteCreateNatInstanceInput,
        operation: &'static str,
        evidence: &mut Evidence,
    ) -> Result<QuoteCreateNatInstanceOutput, Error> {
        let read = Read {
            id: operation,
            ..PRICE
        };
        let body = nat_purchase_body(scope, input);
        if evidence.output_reflects(&body) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let body = serde_json::to_vec(&body).map_err(|_| Error::new(ErrorClass::InvalidInput))?;
        let raw = self
            .exchange(
                ctx,
                &scope.base,
                read,
                super::inventory::Selection::path(&[&scope.zone, &scope.project]),
                (http::Method::POST, body.into()),
                evidence,
            )
            .await?;
        let quote =
            decode_quote(&go_json(&raw), evidence).map_err(|error| screen(error, evidence))?;
        super::mutations::guard_operands(operation, quote, evidence)
    }

    pub(super) async fn check_nat_placement(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        input: &QuoteCreateNatInstanceInput,
        operation: &'static str,
        evidence: &mut Evidence,
    ) -> Result<super::NatPackageOffer, Error> {
        let body = self
            .nat_catalog_request(
                ctx,
                Read {
                    id: operation,
                    ..WHITELIST
                },
                scope,
                &[],
                evidence,
            )
            .await?;
        let invalid = || catalog_invalid(PRICE, evidence);
        let text = go_json(&body);
        let raw = placement_data(&text, evidence).map_err(|()| invalid())?;
        let data: Fields<'_> = raw.required("data").map_err(|()| invalid())?;
        let enabled = data
            .required::<bool>("enabledForAll")
            .map_err(|()| invalid())?;
        if !super::mutations::guard_operands(operation, enabled, evidence)? {
            return Err(local_error(
                ErrorClass::InvalidConfig,
                "requires NAT V3 enabledForAll",
                evidence,
            ));
        }
        let zones = self.nat_zones(ctx, operation, scope, evidence).await?;
        let mut matching = zones
            .iter()
            .filter(|zone| zone.uuid == input.availability_zone_id);
        let zone = matching.next().ok_or_else(|| {
            local_error(
                ErrorClass::InvalidInput,
                "requires one exact availability zone",
                evidence,
            )
        })?;
        if zone.zone_type != "AVAILABILITY" || !zone.is_enabled {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires an enabled AVAILABILITY zone",
                evidence,
            ));
        }
        if matching.next().is_some() {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires one exact availability zone",
                evidence,
            ));
        }
        let offers = self
            .nat_packages(ctx, operation, scope, &input.availability_zone_id, evidence)
            .await?;
        let mut matching = offers
            .into_iter()
            .filter(|offer| offer.uuid == input.package_id);
        let offer = matching
            .next()
            .filter(|offer| offer.currency_unit == "VND")
            .ok_or_else(|| {
                local_error(
                    ErrorClass::InvalidInput,
                    "requires one exact VND package in the availability zone",
                    evidence,
                )
            })?;
        if matching.next().is_some() {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires one exact VND package in the availability zone",
                evidence,
            ));
        }
        self.check_nat_vpc(ctx, scope, &input.vpc_id, operation, evidence)
            .await?;
        Ok(offer)
    }

    pub(super) async fn check_nat_vpc(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        vpc_id: &str,
        operation: &'static str,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let mut seen = HashSet::new();
        let mut totals = None;
        let mut selected = false;
        for page in 1..=50 {
            let query = params(page, 1000);
            let body = self
                .nat_catalog_request(
                    ctx,
                    Read {
                        id: operation,
                        ..VPCS
                    },
                    scope,
                    &[("params", &query)],
                    evidence,
                )
                .await?;
            let text = go_json(&body);
            let invalid = || catalog_invalid(PRICE, evidence);
            let fields = placement_data(&text, evidence).map_err(|()| invalid())?;
            let number = |name| {
                fields
                    .required::<Field<Option<i64>>>(name)
                    .ok()
                    .and_then(|field| field.0)
                    .ok_or_else(invalid)
            };
            let info = PageInfo {
                size: number("size")?,
                pages: number("totalPage")?,
                total: number("total")?,
            };
            if number("page")? != page
                || info.size <= 0
                || !(0..=50).contains(&info.pages)
                || info.total < 0
                || totals.is_some_and(|old| old != info)
            {
                return Err(invalid());
            }
            totals = Some(info);
            let rows: Vec<&RawValue> = match fields.raw("data").map_err(|()| invalid())? {
                Some(raw) => serde_json::from_str(raw.get()).map_err(|_| invalid())?,
                None if info.total == 0 && info.pages == 0 => Vec::new(),
                None => return Err(invalid()),
            };
            if i64::try_from(rows.len()).map_or(true, |len| len > info.size)
                || (info.total == 0 && (!rows.is_empty() || info.pages != 0))
                || (info.total > 0 && (info.pages == 0 || rows.is_empty()))
            {
                return Err(invalid());
            }
            for row in rows {
                let fields: Fields<'_> = serde_json::from_str(row.get()).map_err(|_| invalid())?;
                for name in ["uuid", "projectUuid", "regionId", "regionUuid", "zones"] {
                    fields.raw(name).map_err(|()| invalid())?;
                }
                let zone_rows: Option<Vec<Fields<'_>>> =
                    fields.optional("zones").map_err(|()| invalid())?;
                for zone in zone_rows.into_iter().flatten() {
                    zone.raw("uuid").map_err(|()| invalid())?;
                }
                let vpc: PickerVpc = serde_json::from_str(row.get()).map_err(|_| invalid())?;
                if !PathIds::Strict.allows(&vpc.uuid)
                    || evidence.id_reflects(&vpc.uuid)
                    || vpc.project != scope.project
                    || vpc.region != scope.zone
                    || vpc
                        .region_uuid
                        .as_ref()
                        .is_some_and(|region| *region != scope.zone)
                    || !seen.insert(vpc.uuid.clone())
                {
                    return Err(invalid());
                }
                let mut zones = HashSet::new();
                for zone in vpc.zones.into_iter().flatten() {
                    if !PathIds::Strict.allows(&zone.uuid)
                        || evidence.id_reflects(&zone.uuid)
                        || !zones.insert(zone.uuid)
                    {
                        return Err(invalid());
                    }
                }
                selected |= vpc.uuid == vpc_id;
            }
            let count = i64::try_from(seen.len()).map_err(|_| invalid())?;
            if count > info.total {
                return Err(invalid());
            }
            if page >= info.pages {
                if count != info.total {
                    return Err(invalid());
                }
                evidence.recheck_read_proofs().map_err(|error| *error)?;
                return if selected {
                    Ok(())
                } else {
                    Err(local_error(
                        ErrorClass::NotFound,
                        ": VPC absent from selected scope",
                        evidence,
                    ))
                };
            }
        }
        Err(catalog_invalid(PRICE, evidence))
    }
}

pub(super) fn nat_purchase_body(
    scope: &NatScope,
    input: &QuoteCreateNatInstanceInput,
) -> serde_json::Value {
    json!({"resourceType":"nat","action":"create","resourceInfo":{
        "isPoc": false, "isEnableAutoRenew": false, "isBuyMorePoc": false,
        "natName": input.name, "packageUuid": input.package_id, "vpcUuid": input.vpc_id,
        "regionUuid": scope.zone, "projectUuid": scope.project,
    }})
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn check_input(
    input: &QuoteCreateNatInstanceInput,
    evidence: &Evidence,
) -> Result<(), Error> {
    let path_ids = [
        ("ZoneID", &input.zone_id),
        ("AvailabilityZoneID", &input.availability_zone_id),
        ("PackageID", &input.package_id),
        ("VPCID", &input.vpc_id),
    ];
    for (field, value) in [("Name", &input.name)].into_iter().chain(path_ids) {
        if value.is_empty() {
            return Err(PRICE.nat_input_error(&format!("requires {field}"), evidence));
        }
    }
    if !input.max_price.is_finite() || input.max_price < 0.0 {
        return Err(local_error(
            ErrorClass::InvalidInput,
            "requires finite nonnegative MaxPrice",
            evidence,
        ));
    }
    for (field, id) in path_ids {
        PRICE.nat_path(field, id, evidence)?;
        if evidence.id_reflects(id) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
    }
    if evidence.text_reflects(&input.name) {
        return Err(Error::new(ErrorClass::InvalidInput));
    }
    Ok(())
}

fn local_error(class: ErrorClass, detail: &str, evidence: &Evidence) -> Error {
    let separator = if detail.starts_with(':') { "" } else { " " };
    let message = match class {
        ErrorClass::InvalidConfig => format!("vngcloud: invalid config: {OP}{separator}{detail}"),
        ErrorClass::NotFound => format!("vngcloud: resource not found: {OP}{separator}{detail}"),
        ErrorClass::Unpriced => format!(
            "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {OP}"
        ),
        _ => format!("vngcloud: invalid input: {OP}{separator}{detail}"),
    };
    super::wire::nat_diagnostic(class, message, evidence)
}

fn placement_data<'a>(text: &'a str, evidence: &Evidence) -> Result<Fields<'a>, ()> {
    if reflects(text, evidence) {
        return Err(());
    }
    let fields = Fields::decode(text)?;
    if !fields.required::<bool>("success")? {
        return Err(());
    }
    for field in ["code", "page", "size", "totalPage", "total"] {
        fields.raw(field)?;
    }
    Ok(fields)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn decode_quote(text: &str, evidence: &Evidence) -> Result<QuoteCreateNatInstanceOutput, Error> {
    let invalid = || catalog_invalid(PRICE, evidence);
    let raw = catalog_data(text, 0, evidence).map_err(|()| invalid())?;
    let fields: Fields<'_> = serde_json::from_str(raw.get()).map_err(|_| invalid())?;
    let amount = |name| {
        fields
            .required::<Field<Option<f64>>>(name)
            .map_err(|()| invalid())
            .map(|field| field.0)
    };
    let optimum_price = amount("optimumPrice")?.ok_or_else(invalid)?;
    let original_price = amount("originalPrice")?.ok_or_else(invalid)?;
    let discount_price = amount("discountPrice")?.ok_or_else(invalid)?;
    let discount_percent = amount("discountPercent")?;
    let properties = decode_rows(
        fields.required("propertiesPrice").map_err(|()| invalid())?,
        &["optimumPrice", "monthlyPrice", "name", "description"],
        |row, _: &NetworkPriceProperty| {
            row.raw("discountPercent")?.ok_or(())?;
            row.raw("currentPrice")?.ok_or(())?;
            Ok(())
        },
        evidence,
    )
    .map_err(|()| invalid())?;
    if optimum_price <= 0.0 {
        return Err(local_error(ErrorClass::Unpriced, "", evidence));
    }
    let output = NetworkQuoteOutput {
        optimum_price,
        original_price,
        discount_price,
        discount_percent,
        properties,
        monthly_price: optimum_price,
        total_price: optimum_price,
        currency: "VND".into(),
    };
    if serde_json::to_string(&output).is_ok_and(|text| !reflects(&text, evidence)) {
        Ok(output)
    } else {
        Err(invalid())
    }
}
