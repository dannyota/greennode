use std::collections::HashSet;
use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::billing_resources::{self, AutoRenewInfo, RenewPeriod, Setting};
use crate::wire::product::{
    evidence::Evidence,
    guard::{Guarded, guarded_poll},
    options::Options,
};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::mutations::{
    Destination, Mutation, check_context, check_quote, input_error, not_sent, tick, typed_error,
};
use super::nat_catalog::NatScope;
use super::nat_quote::{self, nat_purchase_body};
use super::nat_writes::{definite_refusal, invalid, nat_ack, nat_error, nat_failure};
use super::wire::{Read, Style};
use super::{Client, NatInstance, QuoteCreateNatInstanceInput};

const OP: &str = "network.CreateNATInstance";
const ROUTE: &str = "vnetwork/v1/{}/{}/nats";
const READ: Read = Read::new(OP, ROUTE, Style::NatCatalog);
const ORDER: Mutation = Mutation::new(OP, ROUTE, Method::POST, &[StatusCode::CREATED])
    .protected(false, "NAT response withheld");
const ORDER_WAIT: Duration = Duration::from_secs(900);
const PROVISIONING_INTERVAL: Duration = Duration::from_secs(5);
const RENEWAL_WAIT: Duration = Duration::from_secs(30);
const RENEWAL_INTERVAL: Duration = Duration::from_secs(2);

/// Selects the NAT name, placement, package, and purchase price cap.
pub type CreateNatInstanceInput = QuoteCreateNatInstanceInput;

/// The observed NAT, quote, and billing renewal state.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateNatInstanceOutput {
    /// The observed NAT, when known.
    #[serde(rename = "NATInstance")]
    pub nat_instance: Option<NatInstance>,
    /// Order identifier when returned by the provider.
    #[serde(rename = "OrderID")]
    pub order_id: String,
    /// Quoted monthly amount.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: f64,
    /// Quoted order total.
    #[serde(rename = "TotalPrice")]
    pub total_price: f64,
    /// Quote currency.
    #[serde(rename = "Currency")]
    pub currency: String,
    /// Last confirmed billing renewal state.
    #[serde(rename = "AutoRenew")]
    pub auto_renew: Option<bool>,
}

crate::wire::product::output::public_fields!(CreateNatInstanceOutput {
    nat_instance,
    order_id,
    monthly_price,
    total_price,
    currency,
    auto_renew
});

struct Purchase {
    scope: NatScope,
    sku: String,
    baseline: HashSet<String>,
}

#[derive(Clone, Copy)]
struct BillingState {
    channel: i64,
    auto_renew: bool,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Purchases a NAT and waits for the billing resource to confirm renewal is off.
    pub async fn create_nat_instance(
        &self,
        ctx: &RequestContext,
        input: CreateNatInstanceInput,
    ) -> Result<CreateNatInstanceOutput, Failure<CreateNatInstanceOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self
                .request_evidence_from(ctx, Evidence::for_write())
                .map_err(not_sent)?;
            self.nat_write_auth(OP, &evidence).map_err(not_sent)?;
            nat_quote::check_input(&input, &evidence)
                .map_err(|error| not_sent(nat_error(OP, error, &evidence)))?;
            let purchase = Guarded::read_preflight(
                &mut evidence,
                async |evidence| self.nat_purchase(ctx, &input, evidence).await,
                |error| *error,
            )
            .await
            .map_err(|error| not_sent(nat_error(OP, error, &evidence)))?
            .into_checked(&evidence)
            .map_err(|error| not_sent(*error))?;
            let quote = self
                .nat_purchase_price(ctx, &purchase.scope, &input, OP, &mut evidence)
                .await
                .map_err(|error| not_sent(nat_error(OP, error, &evidence)))?;
            check_quote(
                OP,
                "total",
                quote.total_price,
                input.max_price,
                &mut evidence,
                &[quote.monthly_price],
            )
            .map_err(not_sent)?;
            let mut output = CreateNatInstanceOutput {
                nat_instance: None,
                order_id: String::new(),
                monthly_price: quote.monthly_price,
                total_price: quote.total_price,
                currency: quote.currency,
                auto_renew: None,
            };
            let scope = &purchase.scope;
            let mut body = nat_purchase_body(scope, &input);
            body["tagDetails"] = json!([]);
            let clock = self.session.clock();
            let order_ctx = ctx.child_with_deadline(clock.monotonic_now() + ORDER_WAIT);
            let order = self
                .mutate::<CreateNatInstanceOutput>(
                    &order_ctx,
                    &ORDER,
                    Destination {
                        base: &scope.base,
                        path: &[&scope.zone, &scope.project],
                        options: Options::DEFAULT,
                    },
                    Some(&body),
                    &mut evidence,
                )
                .await;
            let order_error = match order {
                Ok(reply) => reply
                    .decode_projection(|body| nat_ack(OP, body, &evidence).map_err(Box::new))
                    .and_then(|()| {
                        let receipt = reply
                            .clean_receipt()
                            .ok_or_else(|| Box::new(invalid(OP, &evidence)))?;
                        receipt.guard(()).retain(&mut evidence)
                    })
                    .err()
                    .map(|error| *error),
                Err(failure) if failure.write_dispatch() == WriteDispatch::NotSent => {
                    return Err(failure);
                }
                Err(failure) => Some(failure.into_parts().0),
            };
            let wait = ctx.child_with_deadline(clock.monotonic_now() + ORDER_WAIT);
            if order_error.as_ref().is_some_and(definite_refusal) {
                let checkpoint = evidence.guard_checkpoint();
                let observed = self
                    .nat_write_inventory(&wait, scope, READ, &mut evidence)
                    .await
                    .and_then(|rows| {
                        check_context(&wait, clock)?;
                        Ok(rows.iter().any(|row| {
                            row.nat_name == input.name && !purchase.baseline.contains(&row.uuid)
                        }))
                    });
                evidence.discard_guards_after(checkpoint);
                match observed {
                    Ok(false) => {
                        if let Some(error) = order_error {
                            return Err(Failure::new(
                                error,
                                Some(output),
                                WriteDispatch::MayHaveBeenSent,
                            ));
                        }
                    }
                    Ok(true) => {}
                    Err(error) => {
                        return Err(nat_failure(OP, order_error, Some(error), output, &evidence));
                    }
                }
            }
            match self
                .wait_nat_order(
                    &wait,
                    &purchase,
                    &input,
                    &mut output,
                    order_error.is_none(),
                    &mut evidence,
                )
                .await
            {
                Ok(()) => Ok(output),
                Err(error) if error.has_class(ErrorClass::Failed) => Err(Failure::new(
                    error,
                    Some(output),
                    WriteDispatch::MayHaveBeenSent,
                )),
                Err(error) => Err(nat_failure(OP, order_error, Some(error), output, &evidence)),
            }
        })
    }

    async fn nat_purchase(
        &self,
        ctx: &RequestContext,
        input: &CreateNatInstanceInput,
        evidence: &mut Evidence,
    ) -> Result<Purchase, Error> {
        let scope = self
            .nat_scope(ctx, READ, input.zone_id.clone(), evidence, false)
            .await?;
        let offer = self
            .check_nat_placement(ctx, &scope, input, OP, evidence)
            .await?;
        let rows = self
            .nat_write_inventory(ctx, &scope, READ, evidence)
            .await?;
        if rows
            .iter()
            .any(|row| row.nat_name == input.name || row.vpc.uuid == input.vpc_id)
        {
            return Err(input_error(OP, ": NAT name or VPC already has a NAT"));
        }
        Ok(Purchase {
            scope,
            sku: offer.billing_sku,
            baseline: rows.into_iter().map(|row| row.uuid).collect(),
        })
    }

    async fn wait_nat_order(
        &self,
        ctx: &RequestContext,
        purchase: &Purchase,
        input: &CreateNatInstanceInput,
        output: &mut CreateNatInstanceOutput,
        accepted: bool,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let clock = self.session.clock();
        guarded_poll!(evidence, {
            check_context(ctx, clock)?;
            let result = self
                .nat_write_inventory(ctx, &purchase.scope, READ, evidence)
                .await;
            if result.as_ref().is_err_and(|error| {
                error
                    .api_error()
                    .is_some_and(|api| api.code() == "InvalidResponse")
            }) {
                output.nat_instance = None;
            }
            let rows = result?;
            let mut candidates = rows
                .into_iter()
                .filter(|row| row.nat_name == input.name && !purchase.baseline.contains(&row.uuid));
            if let Some(row) = candidates.next() {
                if candidates.next().is_some()
                    || row.vpc.uuid != input.vpc_id
                    || row.nat_package.uuid != input.package_id
                    || output
                        .nat_instance
                        .as_ref()
                        .is_some_and(|previous| previous.uuid != row.uuid)
                {
                    output.nat_instance = None;
                    return Err(invalid(OP, evidence));
                }
                let row = output.nat_instance.insert(row);
                check_context(ctx, clock)?;
                if row.status == "ERROR" {
                    return Err(if accepted {
                        typed_error(
                            ErrorClass::Failed,
                            "WriteFailed",
                            "network.CreateNATInstance: NAT reached ERROR",
                            evidence,
                        )
                    } else {
                        Error::new(ErrorClass::NotSettled)
                    });
                }
                if row.status == "ACTIVE" {
                    let observed = self
                        .nat_billing(ctx, &row.uuid, &purchase.sku, evidence)
                        .await;
                    if let Err(error) = evidence.recheck_read_proofs() {
                        output.nat_instance = None;
                        return Err(*error);
                    }
                    if let Some(state) = observed? {
                        output.auto_renew = Some(state.auto_renew);
                        check_context(ctx, clock)?;
                        if !accepted {
                            return Err(Error::new(ErrorClass::NotSettled));
                        }
                        if !state.auto_renew {
                            return Ok(());
                        }
                        return self
                            .disable_nat_renewal(
                                ctx,
                                &row.uuid,
                                &purchase.sku,
                                state.channel,
                                &mut output.auto_renew,
                                evidence,
                            )
                            .await;
                    }
                }
            }
            tick(ctx, clock, PROVISIONING_INTERVAL).await?;
        })
    }

    async fn nat_billing(
        &self,
        ctx: &RequestContext,
        id: &str,
        sku: &str,
        evidence: &mut Evidence,
    ) -> Result<Option<BillingState>, Error> {
        let resources = billing_resources::list(&self.session, ctx, evidence)
            .await
            .map_err(|error| *error)?;
        evidence.recheck_read_proofs().map_err(|error| *error)?;
        let mut matches = resources
            .rows(evidence)
            .map_err(|error| *error)?
            .filter(|(row, _)| {
                row.product == "vserver" && row.artifact_type == "nat" && row.artifact_id == id
            });
        let Some((row, period)) = matches.next() else {
            return Ok(None);
        };
        let [element] = row.billing_elements.as_slice() else {
            return Err(invalid(OP, evidence));
        };
        let channel = row.channel.ok_or_else(|| invalid(OP, evidence))?;
        let auto_renew = match (row.renew_type.as_str(), period) {
            ("MANUAL", RenewPeriod::Null) => false,
            ("AUTO-RENEW", RenewPeriod::Value(1)) => true,
            _ => return Err(invalid(OP, evidence)),
        };
        if matches.next().is_some()
            || row
                .status
                .as_deref()
                .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
                .as_deref()
                != Some("active")
            || row.billing_type != "PREPAID"
            || row.is_renewing == Some(true)
            || element.sku != sku
            || element
                .quantity
                .as_deref()
                .and_then(|raw| serde_json::from_str::<f64>(raw.get()).ok())
                != Some(1.0)
        {
            return Err(invalid(OP, evidence));
        }
        Ok(Some(BillingState {
            channel,
            auto_renew,
        }))
    }

    async fn disable_nat_renewal(
        &self,
        ctx: &RequestContext,
        id: &str,
        sku: &str,
        channel: i64,
        auto_renew: &mut Option<bool>,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let account = billing_resources::account(&self.session, ctx, evidence)
            .await
            .map_err(|error| *error)?;
        evidence.recheck_read_proofs().map_err(|error| *error)?;
        let clock = self.session.clock();
        check_context(ctx, clock)?;
        let sent = billing_resources::put(
            &self.session,
            ctx,
            OP,
            &account,
            Setting {
                product: "vserver",
                artifact_type: "nat",
                artifact_id: id,
                channel,
                auto_renew_info: AutoRenewInfo {
                    is_enable: false,
                    period: 43200,
                },
            },
            evidence,
        )
        .await;
        if let Err(failure) = &sent
            && failure.write_dispatch() == WriteDispatch::NotSent
        {
            return Err(sent
                .err()
                .map_or_else(|| invalid(OP, evidence), |failure| failure.into_parts().0));
        }
        let send_error = sent.err().map(|failure| failure.into_parts().0);
        let confirm = ctx.child_with_deadline(clock.monotonic_now() + RENEWAL_WAIT);
        let result = async {
            guarded_poll!(evidence, {
                check_context(&confirm, clock)?;
                let observed = self
                    .nat_billing(&confirm, id, sku, evidence)
                    .await?
                    .ok_or_else(|| invalid(OP, evidence))?;
                *auto_renew = Some(observed.auto_renew);
                if send_error.is_some() || !observed.auto_renew {
                    return check_context(&confirm, clock);
                }
                tick(&confirm, clock, RENEWAL_INTERVAL).await?;
            })
        }
        .await;
        match (send_error, result) {
            (None, result) => result,
            (Some(error), Ok(())) => Err(error),
            (Some(error), Err(confirm)) => Err(error.with_cause(confirm)),
        }
    }
}
