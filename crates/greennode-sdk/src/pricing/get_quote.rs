use super::{ACTION_CREATE, ACTION_RESIZE, Client};
use crate::quote::{
    input,
    request::{self, Product, Request, decode},
};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

const OP: &str = "pricing.GetQuote";

/// Describes a resource to quote on the regional gateway.
#[derive(Clone, Debug)]
pub struct GetQuoteInput {
    /// Resource type sent to the quote gateway.
    pub resource_type: String,
    /// Quote action. An empty action selects `create`.
    pub action: String,
    /// Optional gateway resource details.
    pub resource_info: Option<BTreeMap<String, Value>>,
}

impl GetQuoteInput {
    /// Creates an input for the given resource type using the default action.
    pub fn new(resource_type: impl Into<String>) -> Self {
        Self {
            resource_type: resource_type.into(),
            action: String::new(),
            resource_info: None,
        }
    }
}

/// Current account quote. A negative resize amount indicates an expected credit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetQuoteOutput {
    /// Final amount.
    pub optimum_price: f64,
    /// Amount before discount.
    pub original_price: f64,
    /// Discount amount.
    pub discount_price: f64,
    /// Discount percentage.
    pub discount_percent: f64,
    /// Quote line items.
    pub properties: Vec<PriceProperty>,
}

/// One line of a regional quote.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PriceProperty {
    /// Line name.
    pub name: String,
    /// Line description.
    pub description: String,
    /// Final line amount.
    pub optimum_price: f64,
    /// Monthly line amount.
    pub monthly_price: f64,
    /// Current line amount when provided.
    pub current_price: Option<f64>,
    /// Discount percentage.
    pub discount_percent: f64,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes a creation or resize without placing an order.
    pub async fn get_quote(
        &self,
        ctx: &RequestContext,
        input: GetQuoteInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        self.quote_with_evidence(ctx, input, &mut Evidence::new())
            .await
    }

    pub(crate) async fn quote_with_evidence(
        &self,
        ctx: &RequestContext,
        input: GetQuoteInput,
        evidence: &mut Evidence,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
        if input.resource_type.is_empty() {
            return Err(failure(Error::new(ErrorClass::InvalidInput).with_cause(
                request::api(
                    "",
                    None,
                    "",
                    "greennode: invalid input: pricing.GetQuote: ResourceType is required",
                    false,
                ),
            )));
        }
        let action = if input.action.is_empty() {
            ACTION_CREATE
        } else {
            &input.action
        };
        let body = input::regional(&input.resource_type, action, input.resource_info.as_ref());
        let raw = request::exchange(
            &self.session,
            ctx,
            evidence,
            Request {
                operation: OP,
                template: "v1/price",
                product: Product::Portal,
                path: &[],
                query: &[],
                body: Some(&body),
            },
            None,
        )
        .await;
        let numeric = evidence.credentials().has_numeric_product_credential();
        match raw {
            Err(error) => {
                let observed = error
                    .detail::<http::StatusCode>()
                    .map(|status| status.as_u16())
                    .or_else(|| error.api_error().and_then(|api| api.status()));
                if numeric || observed.is_some_and(|status| (200..400).contains(&status)) {
                    Err(failure(
                        decode::invalid_quote(OP, observed).with_cause(*error),
                    ))
                } else {
                    Err(failure(*error))
                }
            }
            Ok(_) if numeric => Err(failure(decode::invalid_quote(OP, Some(200)))),
            Ok(raw) => {
                let output = crate::quote::catalog::value(OP, &raw)
                    .ok()
                    .and_then(|value| project(&value, action).ok());
                let output = output.filter(|result| {
                    serde_json::to_value(result).is_ok_and(|value| !evidence.reflected_in(&value))
                });
                output.ok_or_else(|| failure(decode::invalid_quote(OP, Some(200))))
            }
        }
    }
}

fn project(raw: &Value, action: &str) -> Result<GetQuoteOutput, Box<Error>> {
    let obj = decode::quote_object(OP, raw)?;
    let amount = |name| decode::numeric(OP, obj, name).map(Option::unwrap_or_default);
    let optimum_price = decode::numeric(OP, obj, "optimumPrice")?
        .filter(|amount| *amount >= 0.0 || action == ACTION_RESIZE)
        .ok_or_else(|| Box::new(decode::invalid_quote(OP, Some(200))))?;
    let properties = decode::properties(OP, obj)?
        .iter()
        .map(|row| {
            if row.is_null() {
                return Ok(PriceProperty::default());
            }
            let line = decode::line(OP, row)?;
            let amount = |name| decode::numeric(OP, line, name).map(Option::unwrap_or_default);
            Ok(PriceProperty {
                name: decode::string(OP, line, "name")?,
                description: decode::string(OP, line, "description")?,
                optimum_price: amount("optimumPrice")?,
                monthly_price: amount("monthlyPrice")?,
                current_price: decode::numeric(OP, line, "currentPrice")?,
                discount_percent: amount("discountPercent")?,
            })
        })
        .collect::<Result<_, Box<Error>>>()?;
    Ok(GetQuoteOutput {
        optimum_price,
        original_price: amount("originalPrice")?,
        discount_price: amount("discountPrice")?,
        discount_percent: amount("discountPercent")?,
        properties,
    })
}
