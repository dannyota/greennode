use serde::Serialize;
use serde_json::{json, value::RawValue};

use crate::model::wire::{Field, go_json};
use crate::quote::request::{self, Product, Request};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext};

use super::endpoint_quote_scope::{EndpointQuote, Offer, hex24, uuid};
use super::inventory::screen;
use super::nat_catalog::catalog_data;
use super::wire::{Fields, Read, Style};
use super::{Client, failure};

pub(super) const OP: &str = "network.QuoteCreateEndpoint";
const PRICE: Read = Read::new(OP, "vnetwork/billing/v1/{}/{}/price", Style::EndpointQuote);

/// Selects a private Gateway endpoint and its placement to quote.
#[derive(Clone, Debug)]
pub struct QuoteCreateEndpointInput {
    /// Proposed endpoint name.
    pub name: String,
    /// VPC in the form net-UUID.
    pub vpc_id: String,
    /// Subnet in the form sub-UUID.
    pub subnet_id: String,
    /// Optional availability-zone selector.
    pub availability_zone_id: String,
    /// Optional category selector.
    pub category_id: String,
    /// Optional Gateway service selector.
    pub service_id: String,
    /// Optional package selector.
    pub package_id: String,
    /// Optional creation limit. The quote validates the number and ignores its value.
    pub max_price: f64,
}

impl QuoteCreateEndpointInput {
    /// Selects the endpoint name, VPC, and subnet with the default offer.
    pub fn new(
        name: impl Into<String>,
        vpc_id: impl Into<String>,
        subnet_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            availability_zone_id: String::new(),
            category_id: String::new(),
            service_id: String::new(),
            package_id: String::new(),
            max_price: 0.0,
        }
    }
}

/// Monthly private Gateway endpoint quote.
#[derive(Clone, PartialEq, Serialize)]
pub struct QuoteCreateEndpointOutput {
    /// Quoted monthly amount.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: f64,
    /// Quote currency.
    #[serde(rename = "Currency")]
    pub currency: String,
}

crate::wire::product::output::public_fields!(QuoteCreateEndpointOutput {
    monthly_price,
    currency
});

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Quotes an eligible Gateway endpoint without placing an order.
    pub async fn quote_create_endpoint(
        &self,
        ctx: &RequestContext,
        input: QuoteCreateEndpointInput,
    ) -> Result<QuoteCreateEndpointOutput, Failure<QuoteCreateEndpointOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            check_input(&input, &evidence).map_err(|error| failure(screen(error, &evidence)))?;
            let mut quote = EndpointQuote::new(self, ctx, &mut evidence)
                .await
                .map_err(failure)?;
            let offer = quote
                .offer(&input)
                .await
                .map_err(|error| failure(screen(error, quote.evidence)))?;
            quote
                .check_placement(&input, &offer)
                .await
                .map_err(|error| failure(screen(error, quote.evidence)))?;
            quote
                .endpoint_price(&input, &offer)
                .await
                .map_err(|error| failure(screen(error, quote.evidence)))
        })
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl EndpointQuote<'_> {
    async fn endpoint_price(
        &mut self,
        input: &QuoteCreateEndpointInput,
        offer: &Offer,
    ) -> Result<QuoteCreateEndpointOutput, Error> {
        let body = json!({"resourceType":"endpoint","action":"create","resourceInfo":{
            "isBuyMorePoc": false, "isPoc": false, "isEnableAutoRenew": false,
            "endpointName": input.name, "categoryUuid": offer.category, "serviceUuid": offer.service,
            "packageUuid": offer.package, "vpcUuid": input.vpc_id, "subnetUuid": input.subnet_id,
            "regionUuid": self.zone, "projectUuid": self.project, "enableAZ": true, "enableDnsName": false,
        }});
        if self.evidence.output_reflects(&body) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let raw = request::exchange(
            &self.client.session,
            self.ctx,
            self.evidence,
            Request {
                operation: OP,
                template: PRICE.template,
                product: Product::Vnetwork(&self.base),
                path: &[&self.zone, &self.project],
                query: &[],
                body: Some(&body),
            },
            None,
        )
        .await
        .map_err(|error| endpoint_error(*error, self.evidence))?;
        let text = go_json(raw.get().as_bytes());
        let invalid = || PRICE.invalid(self.evidence);
        let raw = catalog_data(&text, 0, self.evidence).map_err(|()| invalid())?;
        let quote: Fields<'_> = serde_json::from_str(raw.get()).map_err(|_| invalid())?;
        let amount = |row: &Fields<'_>, name| {
            row.required::<Field<Option<f64>>>(name)
                .ok()
                .and_then(|field| field.0)
                .ok_or_else(invalid)
        };
        let optimum = amount(&quote, "optimumPrice")?;
        let rows: Vec<Fields<'_>> = quote.required("propertiesPrice").map_err(|()| invalid())?;
        let [property] = rows.as_slice() else {
            return Err(invalid());
        };
        let name: String = property.required("name").map_err(|()| invalid())?;
        if optimum < 0.0
            || amount(property, "optimumPrice")? != optimum
            || amount(property, "monthlyPrice")? != optimum
            || name != offer.package_name
        {
            return Err(invalid());
        }
        if optimum == 0.0 {
            return Err(local_error(ErrorClass::Unpriced, ""));
        }
        if self.evidence.text_reflects("VND")
            || self.evidence.text_reflects(&optimum.to_string())
            || crate::output_number::reflects(optimum, self.evidence.credentials(), true)
        {
            return Err(invalid());
        }
        Ok(QuoteCreateEndpointOutput {
            monthly_price: optimum,
            currency: "VND".into(),
        })
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn check_input(input: &QuoteCreateEndpointInput, evidence: &Evidence) -> Result<(), Error> {
    for (field, value) in [
        ("Name", &input.name),
        ("VPCID", &input.vpc_id),
        ("SubnetID", &input.subnet_id),
    ] {
        if value.is_empty() {
            return Err(local_error(
                ErrorClass::InvalidInput,
                &format!("requires {field}"),
            ));
        }
    }
    if !input.max_price.is_finite() || input.max_price < 0.0 {
        return Err(local_error(
            ErrorClass::InvalidInput,
            "requires finite nonnegative MaxPrice",
        ));
    }
    if input.name.trim().is_empty() || input.name.chars().any(char::is_control) {
        return Err(local_error(
            ErrorClass::InvalidInput,
            "requires a nonblank Name without control characters",
        ));
    }
    for (field, value, pattern, form) in [
        (
            "VPCID",
            &input.vpc_id,
            input.vpc_id.strip_prefix("net-").is_some_and(uuid),
            "net-<uuid>",
        ),
        (
            "SubnetID",
            &input.subnet_id,
            input.subnet_id.strip_prefix("sub-").is_some_and(uuid),
            "sub-<uuid>",
        ),
        (
            "CategoryID",
            &input.category_id,
            hex24(&input.category_id),
            "catalog ID",
        ),
        (
            "ServiceID",
            &input.service_id,
            uuid(&input.service_id),
            "catalog ID",
        ),
        (
            "PackageID",
            &input.package_id,
            uuid(&input.package_id),
            "catalog ID",
        ),
        (
            "AvailabilityZoneID",
            &input.availability_zone_id,
            PathIds::Strict.allows(&input.availability_zone_id),
            "availability zone ID",
        ),
    ] {
        if value.is_empty() {
            continue;
        }
        PRICE.path(field, value)?;
        if !pattern {
            return Err(local_error(
                ErrorClass::InvalidInput,
                &format!("requires {field} in the form {form}"),
            ));
        }
        if evidence.id_reflects(value) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
    }
    if evidence.text_reflects(&input.name) {
        return Err(Error::new(ErrorClass::InvalidInput));
    }
    Ok(())
}

pub(super) fn local_error(class: ErrorClass, detail: &str) -> Error {
    let (code, message) = match class {
        ErrorClass::InvalidConfig => (
            "InvalidConfig",
            format!("vngcloud: invalid config: {OP}{detail}"),
        ),
        ErrorClass::NotFound => (
            "NotFound",
            format!("vngcloud: resource not found: {OP}{detail}"),
        ),
        ErrorClass::Unpriced => (
            "Unpriced",
            format!(
                "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {OP}"
            ),
        ),
        _ => (
            "InvalidUsage",
            format!(
                "vngcloud: invalid input: {OP}{}{detail}",
                if detail.starts_with(':') { "" } else { " " }
            ),
        ),
    };
    Error::new(class).with_cause(request::api("", None, code, &message, false))
}

fn endpoint_error(error: Error, evidence: &Evidence) -> Error {
    if error.is_body_too_large()
        || error.api_error().is_some_and(|api| {
            api.status() == Some(200) || api.code() == "InvalidResponse" && api.status().is_none()
        })
    {
        return PRICE.invalid(evidence);
    }
    let error = if let Some(api) = error.api_error() {
        request::api(
            OP,
            api.status(),
            api.code(),
            "endpoint response withheld",
            api.retryable(),
        )
    } else {
        error
    };
    screen(error, evidence)
}

pub(super) fn fields<'a>(raw: &'a RawValue) -> Result<Fields<'a>, ()> {
    serde_json::from_str(raw.get()).map_err(|_| ())
}
