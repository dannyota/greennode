use std::collections::BTreeMap;

use serde_json::json;

use crate::pricing::{GetQuoteInput, GetQuoteOutput};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, Failure, RequestContext};

use super::wire::{invalid_input, screen_input, validate_ids};
use super::{Client, failure};

/// Parameters for pricing a new regional load balancer.
#[derive(Clone, Debug, Default)]
pub struct CreateLoadBalancerInput {
    /// Display name, which does not affect the quote.
    pub name: String,
    /// ID of the package to price.
    pub package_id: String,
    /// Load balancer type, which does not affect the quote.
    pub r#type: String,
    /// Optional scheme. Accepted values are `Internet` and `Internal`.
    pub scheme: String,
    /// Optional subnet identifier. It does not affect the quote.
    pub subnet_id: String,
    /// Zone where the package will be priced.
    pub zone_id: String,
    /// Price limit used when ordering. A quote ignores this field.
    pub max_price: f64,
    /// Settlement option used when ordering. A quote ignores this field.
    pub no_wait: bool,
}

/// Parameters for pricing a new package for an existing regional load balancer.
#[derive(Clone, Debug, Default)]
pub struct ResizeLoadBalancerInput {
    /// ID of the load balancer to price.
    pub load_balancer_id: String,
    /// ID of the target package.
    pub package_id: String,
    /// Price limit used when ordering. A quote ignores this field.
    pub max_price: f64,
    /// Settlement option used when ordering. A quote ignores this field.
    pub no_wait: bool,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes a package and zone without placing an order.
    pub async fn quote_create_load_balancer(
        &self,
        ctx: &RequestContext,
        input: CreateLoadBalancerInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        const OP: &str = "loadbalancer.QuoteCreateLoadBalancer";
        let mut evidence = self.quote_evidence(ctx).map_err(failure)?;
        screen_input(&evidence, [&*input.package_id, &input.zone_id]).map_err(failure)?;
        if input.package_id.is_empty() {
            return Err(failure(invalid_input(&format!("{OP} requires PackageID"))));
        }
        if input.zone_id.is_empty() {
            return Err(failure(invalid_input(&format!("{OP} requires ZoneID"))));
        }
        if !matches!(input.scheme.as_str(), "" | "Internet" | "Internal") {
            return Err(failure(invalid_input(&format!(
                "{OP}: Scheme must be \"Internet\" or \"Internal\""
            ))));
        }
        let ids = [
            ("PackageID", input.package_id.as_str()),
            ("SubnetID", input.subnet_id.as_str()),
        ];
        let ids = if input.subnet_id.is_empty() {
            &ids[..1]
        } else {
            &ids
        };
        validate_ids(OP, ids, PathIds::Strict).map_err(failure)?;
        let details = create_quote_info(&input);
        self.request_quote(ctx, &mut evidence, "create", details)
            .await
    }

    /// Quotes the new package for a load balancer without resizing it.
    pub async fn quote_resize_load_balancer(
        &self,
        ctx: &RequestContext,
        input: ResizeLoadBalancerInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        const OP: &str = "loadbalancer.QuoteResizeLoadBalancer";
        let mut evidence = self.quote_evidence(ctx).map_err(failure)?;
        screen_input(&evidence, [&*input.load_balancer_id, &input.package_id]).map_err(failure)?;
        validate_ids(
            OP,
            &[
                ("LoadBalancerID", &input.load_balancer_id),
                ("PackageID", &input.package_id),
            ],
            PathIds::Strict,
        )
        .map_err(failure)?;
        let details = resize_quote_info(&input);
        self.request_quote(ctx, &mut evidence, "resize", details)
            .await
    }

    pub(super) fn quote_evidence(&self, ctx: &RequestContext) -> Result<Evidence, Box<Error>> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        let mut evidence = Evidence::new();
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        Ok(evidence)
    }

    async fn request_quote(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        action: &str,
        details: BTreeMap<String, serde_json::Value>,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        self.pricing
            .quote_with_evidence(
                ctx,
                GetQuoteInput {
                    resource_type: "load-balancer".into(),
                    action: action.into(),
                    resource_info: Some(details),
                },
                evidence,
            )
            .await
            .map_err(|result| {
                failure(Box::new(
                    result
                        .into_parts()
                        .0
                        .screen_api(&|text| evidence.text_reflects(text)),
                ))
            })
    }
}

pub(super) fn create_quote_info(
    input: &CreateLoadBalancerInput,
) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("packageId".into(), json!(input.package_id)),
        ("zoneId".into(), json!(input.zone_id)),
        ("isBuyMorePoc".into(), json!(false)),
        ("period".into(), json!(1)),
        ("isPoc".into(), json!(false)),
    ])
}

pub(super) fn resize_quote_info(
    input: &ResizeLoadBalancerInput,
) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("loadBalancerId".into(), json!(input.load_balancer_id)),
        ("packageId".into(), json!(input.package_id)),
    ])
}
