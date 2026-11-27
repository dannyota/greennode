use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue};

use crate::model::wire::{Field, go_json, model};
use crate::quote::request::{self, Product, Request};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext};

use super::inventory::screen;
use super::nat_catalog::{NatScope, catalog_data, catalog_invalid, decode_rows, params, reflects};
use super::wire::{Fields, Read, Style};
use super::{Client, failure};

const OP: &str = "network.QuoteCreateNATInstance";
const PRICE: Read = Read::new(OP, "vnetwork/billing/v1/{}/{}/price", Style::NatCatalog);
const WHITELIST: Read = PRICE.at("vnetwork/v1/{}/{}/nats/v3-whitelist", Style::NatCatalog);
const VPCS: Read = PRICE.at("vnetwork/v1/{}/{}/nats/vpcs", Style::NatCatalog);

/// Selects the Public NAT package and placement to quote.
#[derive(Clone, Debug)]
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
    zones: Vec<PickerZone>,
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
            check_input(&input, &evidence).map_err(|error| failure(screen(error, &evidence)))?;
            let scope = self
                .nat_scope(ctx, PRICE, input.zone_id.clone(), &mut evidence, false)
                .await
                .map_err(|error| failure(screen(error, &evidence)))?;
            self.check_nat_placement(ctx, &scope, &input, &mut evidence)
                .await
                .map_err(|error| failure(screen(error, &evidence)))?;
            let body = json!({"resourceType":"nat","action":"create","resourceInfo":{
                "isPoc": false, "isEnableAutoRenew": false, "isBuyMorePoc": false,
                "natName": input.name, "packageUuid": input.package_id, "vpcUuid": input.vpc_id,
                "regionUuid": scope.zone, "projectUuid": scope.project,
            }});
            if evidence.output_reflects(&body) {
                return Err(failure(Error::new(ErrorClass::InvalidInput)));
            }
            let result = request::exchange(
                &self.session,
                ctx,
                &mut evidence,
                Request {
                    operation: OP,
                    template: PRICE.template,
                    product: Product::Vnetwork(&scope.base),
                    path: &[&scope.zone, &scope.project],
                    query: &[],
                    body: Some(&body),
                },
                None,
            )
            .await;
            let raw = result.map_err(|error| failure(quote_error(*error, &evidence)))?;
            decode_quote(&go_json(raw.get().as_bytes()), &evidence)
                .map_err(|error| failure(screen(error, &evidence)))
        })
    }

    async fn check_nat_placement(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        input: &QuoteCreateNatInstanceInput,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let body = self
            .nat_catalog_request(ctx, WHITELIST, scope, &[], evidence)
            .await?;
        let invalid = || catalog_invalid(PRICE, evidence);
        let text = go_json(&body);
        let raw = placement_data(&text, evidence).map_err(|()| invalid())?;
        let data: Fields<'_> = raw.required("data").map_err(|()| invalid())?;
        if !data
            .required::<bool>("enabledForAll")
            .map_err(|()| invalid())?
        {
            return Err(local_error(
                ErrorClass::InvalidConfig,
                "requires NAT V3 enabledForAll",
            ));
        }
        let zones = self.nat_zones(ctx, OP, scope, evidence).await?;
        let mut matching = zones
            .iter()
            .filter(|zone| zone.uuid == input.availability_zone_id);
        let zone = matching.next().ok_or_else(|| {
            local_error(
                ErrorClass::InvalidInput,
                "requires one exact availability zone",
            )
        })?;
        if zone.zone_type != "AVAILABILITY" || !zone.is_enabled {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires an enabled AVAILABILITY zone",
            ));
        }
        if matching.next().is_some() {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires one exact availability zone",
            ));
        }
        let offers = self
            .nat_packages(ctx, OP, scope, &input.availability_zone_id, evidence)
            .await?;
        let mut matching = offers.iter().filter(|offer| offer.uuid == input.package_id);
        if !matching
            .next()
            .is_some_and(|offer| offer.currency_unit == "VND")
            || matching.next().is_some()
        {
            return Err(local_error(
                ErrorClass::InvalidInput,
                "requires one exact VND package in the availability zone",
            ));
        }
        self.check_nat_vpc(ctx, scope, input, evidence).await
    }

    async fn check_nat_vpc(
        &self,
        ctx: &RequestContext,
        scope: &NatScope,
        input: &QuoteCreateNatInstanceInput,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let mut seen = HashSet::new();
        let mut totals = None;
        let mut selected = None;
        for page in 1..=50 {
            let query = params(page, 1000);
            let body = self
                .nat_catalog_request(ctx, VPCS, scope, &[("params", &query)], evidence)
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
                let zone_rows: Vec<Fields<'_>> =
                    fields.required("zones").map_err(|()| invalid())?;
                for zone in zone_rows {
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
                for zone in vpc.zones {
                    if !PathIds::Strict.allows(&zone.uuid)
                        || evidence.id_reflects(&zone.uuid)
                        || !zones.insert(zone.uuid)
                    {
                        return Err(invalid());
                    }
                }
                if vpc.uuid == input.vpc_id {
                    selected = Some(());
                }
            }
            let count = i64::try_from(seen.len()).map_err(|_| invalid())?;
            if count > info.total {
                return Err(invalid());
            }
            if page >= info.pages {
                if count != info.total {
                    return Err(invalid());
                }
                return match selected {
                    Some(()) => Ok(()),
                    None => Err(local_error(
                        ErrorClass::NotFound,
                        ": VPC absent from selected scope",
                    )),
                };
            }
        }
        Err(catalog_invalid(PRICE, evidence))
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn check_input(input: &QuoteCreateNatInstanceInput, evidence: &Evidence) -> Result<(), Error> {
    if input.name.is_empty() {
        return Err(local_error(ErrorClass::InvalidInput, "requires Name"));
    }
    if !input.max_price.is_finite() || input.max_price < 0.0 {
        return Err(local_error(
            ErrorClass::InvalidInput,
            "requires finite nonnegative MaxPrice",
        ));
    }
    for (field, id) in [
        ("ZoneID", &input.zone_id),
        ("AvailabilityZoneID", &input.availability_zone_id),
        ("PackageID", &input.package_id),
        ("VPCID", &input.vpc_id),
    ] {
        PRICE.path(field, id)?;
        if evidence.id_reflects(id) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
    }
    if evidence.text_reflects(&input.name) {
        return Err(Error::new(ErrorClass::InvalidInput));
    }
    Ok(())
}

fn local_error(class: ErrorClass, detail: &str) -> Error {
    let separator = if detail.starts_with(':') { "" } else { " " };
    let (code, message) = match class {
        ErrorClass::InvalidConfig => (
            "InvalidConfig",
            format!("vngcloud: invalid config: {OP}{separator}{detail}"),
        ),
        ErrorClass::NotFound => (
            "NotFound",
            format!("vngcloud: resource not found: {OP}{separator}{detail}"),
        ),
        ErrorClass::Unpriced => (
            "Unpriced",
            format!(
                "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {OP}"
            ),
        ),
        _ => (
            "InvalidUsage",
            format!("vngcloud: invalid input: {OP}{separator}{detail}"),
        ),
    };
    Error::new(class).with_cause(request::api("", None, code, &message, false))
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

fn quote_error(error: Error, evidence: &Evidence) -> Error {
    if error.is_body_too_large()
        || error.api_error().is_some_and(|api| {
            api.status() == Some(200) || api.code() == "InvalidResponse" && api.status().is_none()
        })
    {
        return catalog_invalid(PRICE, evidence);
    }
    let Some(api) = error.api_error() else {
        return screen(error, evidence);
    };
    let code = if api.code() == "InvalidResponse" {
        "InvalidResponse"
    } else {
        api.code()
    };
    screen(
        request::api(
            OP,
            api.status(),
            code,
            "NAT response withheld",
            api.retryable(),
        ),
        evidence,
    )
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
        return Err(local_error(ErrorClass::Unpriced, ""));
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
