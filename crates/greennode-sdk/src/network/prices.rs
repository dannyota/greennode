use serde::Serialize;

use crate::model::pricing::{
    DiscoveryFailure, DiscoveryMetadata, DiscoveryPrice, PricingNote, discovery_notes, note,
};
use crate::quote::{
    catalog::{self, Catalog},
    input,
    request::{self, Product, Request, decode},
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::inventory::screen;
use super::{Client, failure};

const QUOTE: &str = "pricing.GetQuote";
const FLOATING: &str = "https://docs.greennode.ai/vserver/compute-hcm03-1a/vpc/floating-ip.md";
const EXTERNAL: &str =
    "https://docs.greennode.ai/vserver/compute-hcm03-1a/vpc/instance-ip-address.md";

/// Controls request bounds and partial results for network price discovery.
#[derive(Clone, Default)]
pub struct ListNetworkPricesInput {
    /// Request limit. Zero selects 500, with a maximum of 2000.
    pub max_requests: i64,
    /// Whether to retain usable prices when coverage is incomplete.
    pub allow_partial: bool,
}

crate::error::debug_record!(ListNetworkPricesInput {
    max_requests,
    allow_partial
});

/// Network price rows and their discovery coverage.
#[derive(Clone, PartialEq, Serialize)]
pub struct ListNetworkPricesOutput {
    /// Quoted or unpriced network products.
    #[serde(rename = "Items")]
    pub items: Vec<NetworkPrice>,
    /// Discovery coverage and provenance.
    #[serde(rename = "Metadata")]
    pub metadata: DiscoveryMetadata,
}

crate::wire::product::output::public_fields!(ListNetworkPricesOutput { items, metadata });

/// The quoted or unknown charge for one network product.
#[derive(Clone, PartialEq, Serialize)]
pub struct NetworkPrice {
    /// Product kind.
    #[serde(rename = "Kind")]
    pub kind: String,
    /// Product display name.
    #[serde(rename = "Name")]
    pub name: String,
    /// Public IP type, when known.
    #[serde(rename = "Type")]
    pub r#type: String,
    /// Region of the charge.
    #[serde(rename = "Scope")]
    pub scope: String,
    /// Charge unit.
    #[serde(rename = "Unit")]
    pub unit: String,
    /// Quote and published source details.
    #[serde(rename = "Price")]
    pub price: DiscoveryPrice,
}

crate::wire::product::output::public_fields!(NetworkPrice {
    kind,
    name,
    r#type,
    scope,
    unit,
    price
});

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Quotes the three public virtual IP types and reports unknown network charges.
    pub async fn list_network_prices(
        &self,
        ctx: &RequestContext,
        input: ListNetworkPricesInput,
    ) -> Result<ListNetworkPricesOutput, Failure<ListNetworkPricesOutput>> {
        crate::wire::product::exit::finish!(async {
            if !(0..=2000).contains(&input.max_requests) {
                let api = request::api(
                    "",
                    None,
                    "InvalidUsage",
                    "vngcloud: invalid input: discovery: max requests must be between 1 and 2000",
                    false,
                );
                return Err(failure(
                    Error::new(ErrorClass::InvalidInput).with_cause(api),
                ));
            }
            let mut catalog = Catalog::new(&self.session, ctx, input.max_requests);
            catalog
                .project()
                .await
                .map_err(|error| failure(screen(*error, &catalog.evidence)))?;
            catalog
                .preflight(3)
                .map_err(|error| failure(screen(*error, &catalog.evidence)))?;
            let mut items = Vec::new();
            let mut failures = Vec::new();
            let mut first_error = None;
            let mut usable = false;
            for kind in ["public", "public-vm", "public-mkp"] {
                let body = input::public_ip(kind);
                let price = public_ip_price(&mut catalog, &body).await;
                let (price, stop) = match price {
                    Ok(price) => {
                        usable = true;
                        (price, false)
                    }
                    Err(error) => {
                        let error = screen(*error, &catalog.evidence);
                        if !input.allow_partial {
                            return Err(failure(error));
                        }
                        let stop = [
                            ErrorClass::Auth,
                            ErrorClass::RateLimited,
                            ErrorClass::NoCredentials,
                            ErrorClass::CredentialsFile,
                            ErrorClass::Cancelled,
                            ErrorClass::DeadlineExceeded,
                            ErrorClass::EstimateLimit,
                        ]
                        .into_iter()
                        .any(|class| error.has_class(class));
                        let api = error.api_error();
                        let status = api.and_then(ApiError::status);
                        let code = if api.is_some_and(|api| api.code() == "InvalidResponse") {
                            "InvalidResponse".into()
                        } else if api.is_some() {
                            ApiError::new(
                                QUOTE,
                                status,
                                crate::error::SafeText::new("", &[]),
                                crate::error::SafeText::new("", &[]),
                                false,
                            )
                            .code()
                            .to_owned()
                        } else {
                            "DiscoveryIncomplete".into()
                        };
                        failures.push(DiscoveryFailure {
                            operation: QUOTE.into(),
                            item_key: kind.into(),
                            status: i64::from(status.unwrap_or(0)),
                            code,
                        });
                        first_error.get_or_insert(error);
                        (
                            DiscoveryPrice {
                                status: "failed".into(),
                                source: QUOTE.into(),
                                ..DiscoveryPrice::default()
                            },
                            stop,
                        )
                    }
                };
                items.push(NetworkPrice {
                    kind: "public-vip".into(),
                    name: format!("Public Virtual IP ({kind})"),
                    r#type: kind.into(),
                    scope: self.region.clone(),
                    unit: "IP-month".into(),
                    price,
                });
                if stop {
                    break;
                }
            }
            if !usable && let Some(error) = first_error {
                return Err(failure(screen(error, &catalog.evidence)));
            }
            for (kind, name, source) in [
                ("floating-ip", "Floating IP", FLOATING),
                ("external-interface", "External Interface", EXTERNAL),
                ("reservation", "Floating IP reservation", FLOATING),
            ] {
                items.push(NetworkPrice {
                    kind: kind.into(),
                    name: name.into(),
                    r#type: String::new(),
                    scope: self.region.clone(),
                    unit: "IP-month".into(),
                    price: DiscoveryPrice {
                        status: "unknown".into(),
                        source: source.into(),
                        ..DiscoveryPrice::default()
                    },
                });
            }
            items.sort_by(|a, b| a.r#type.cmp(&b.r#type));
            let output = ListNetworkPricesOutput {
                items,
                metadata: DiscoveryMetadata {
                    region: self.region.clone(),
                    complete: failures.is_empty(),
                    requests: i64::try_from(catalog.requests()).unwrap_or_default(),
                    quoted_at: self.session.clock().utc_now(),
                    failures,
                    notes: notes(),
                    excluded_groups: None,
                },
            };
            let reflected_amount = output.items.iter().any(|item| {
                [
                    item.price.monthly_price,
                    item.price.original_price,
                    item.price.discount_price,
                    item.price.discount_percent,
                ]
                .into_iter()
                .flatten()
                .any(|amount| catalog.evidence.reflects_fixed_two(amount))
            });
            if reflected_amount
                || !serde_json::to_value(&output)
                    .is_ok_and(|value| !catalog.evidence.output_reflects(&value))
            {
                return Err(failure(screen(
                    *request::invalid(QUOTE, 200),
                    &catalog.evidence,
                )));
            }
            if output.metadata.complete {
                Ok(output)
            } else {
                let error = Error::new(ErrorClass::DiscoveryIncomplete);
                let error = first_error.map_or_else(
                    || Error::new(ErrorClass::DiscoveryIncomplete),
                    |cause| error.with_cause(screen(cause, &catalog.evidence)),
                );
                Err(Failure::new(error, Some(output), WriteDispatch::NotAWrite))
            }
        })
    }
}

async fn public_ip_price(
    catalog: &mut Catalog<'_>,
    body: &serde_json::Value,
) -> Result<DiscoveryPrice, Box<Error>> {
    let raw = catalog
        .exchange(Request {
            operation: QUOTE,
            template: "v1/price",
            product: Product::Portal,
            path: &[],
            query: &[],
            body: Some(body),
        })
        .await?;
    let value = catalog::value(QUOTE, &raw)?;
    let mut price = decode::monthly(QUOTE, &value)?;
    price.currency = "VND".into();
    if price
        .monthly_price
        .is_some_and(|amount| catalog.evidence.reflects_fixed_two(amount))
    {
        return Err(request::invalid(QUOTE, 200));
    }
    catalog.rows(1)?;
    Ok(price)
}

fn notes() -> Vec<PricingNote> {
    let mut notes: Vec<_> = discovery_notes()
        .into_iter()
        .filter(|note| note.scope == "network" || note.topic == "account-scope")
        .collect();
    notes.extend([
        note("quote-scope", "public-vip", "Each public-VIP type has a separate account quote. Equal amounts do not establish interchangeable products.", "", "observed"),
        note("unknown-ip-charges", "network", "Floating IP, external-interface, and reservation charges remain unknown. Reservation affects Floating IP billing; detached External Interfaces keep billing until release.", EXTERNAL, "published"),
        note("nat", "network", "NAT costs are excluded. Use network list-nat-zones, list-nat-packages, and quote-create-nat-instance separately.", "", "unverified"),
    ]);
    notes
}
