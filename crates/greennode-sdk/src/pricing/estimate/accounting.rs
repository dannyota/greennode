use super::{
    EstimateArithmeticError, EstimateInput, EstimateItem, EstimateOutput, OP, Run, Shared, failed,
    failure_code,
};
use crate::model::pricing::{
    DiscoveryFailure, DiscoveryMetadata, PricingNote, discovery_notes, note,
};
use crate::pricing::{EstimateCharge, EstimateCoverage, EstimateTotal};
use crate::quote::request::decode::Quote;
use crate::{Error, ErrorClass};
use std::sync::Arc;
use time::OffsetDateTime;

pub(super) fn initialize(
    input: &EstimateInput,
    region: &str,
    now: OffsetDateTime,
) -> EstimateOutput {
    let mut items = Vec::new();
    let mut add =
        |key: &str, kind: &str, server_key: &str, region: &str, zone_id: &str, quantity| {
            items.push(EstimateItem {
                key: key.into(),
                kind: kind.into(),
                server_key: server_key.into(),
                region: region.into(),
                zone_id: zone_id.into(),
                quantity,
                price: failed(),
                ..EstimateItem::default()
            });
        };
    for row in &input.servers {
        add(&row.key, "server", "", region, &row.zone_id, row.quantity);
    }
    for row in &input.volumes {
        add(
            &row.key,
            "volume",
            &row.server_key,
            region,
            &row.zone_id,
            row.quantity,
        );
    }
    for row in &input.public_ips {
        add(&row.key, &row.kind, "", region, "", row.quantity);
    }
    for row in &input.storage {
        add(&row.key, "object-storage", "", "", "", row.quantity);
    }
    let mut coverage = Vec::new();
    for topic in [
        "bandwidth-packages",
        "software-licenses",
        "nat",
        "load-balancers",
        "backups",
        "snapshots",
        "payg",
        "negotiated-discounts-credits",
    ] {
        coverage.push(cover(topic, "setup", "excluded", "outside-estimate-scope"));
    }
    for item in &items {
        if item.kind == "server" {
            coverage.push(cover(
                "vserver-traffic",
                &item.key,
                "unknown",
                "usage-not-priced",
            ));
            if input.public_ips.is_empty() {
                coverage.push(cover(
                    "public-ip",
                    &item.key,
                    "excluded",
                    "no IP product specified; any required IP cost is excluded",
                ));
            }
        }
        if item.kind == "object-storage" {
            coverage.push(cover(
                "object-egress-requests",
                &item.key,
                "unknown",
                "usage-not-priced",
            ));
        }
    }
    if input.servers.is_empty() {
        coverage.push(cover(
            "vserver-traffic",
            "setup",
            "excluded",
            "outside-estimate-scope",
        ));
    }
    if input.storage.is_empty() {
        coverage.push(cover(
            "object-egress-requests",
            "setup",
            "excluded",
            "outside-estimate-scope",
        ));
    }
    let mut notes: Vec<_> = discovery_notes()
        .into_iter()
        .filter(|item| {
            matches!(
                item.topic.as_str(),
                "account-scope" | "terms" | "egress" | "bandwidth"
            )
        })
        .collect();
    notes.push(note(
        "estimate",
        "setup",
        "monthly, VAT as returned, account-scoped estimate; not a commitment",
        "",
        "projection",
    ));
    notes.push(note("quantity", "setup", "Quantities project independent unit quotes without bulk discounts or shared allowances. Quotes do not establish deployment feasibility or capacity.", "", "projection"));
    EstimateOutput {
        items,
        coverage,
        total: EstimateTotal {
            currency: "VND".into(),
            tax: "as-returned".into(),
            basis: "sum-of-independent-monthly-quotes".into(),
            scope: "specified-recurring-components".into(),
            ..EstimateTotal::default()
        },
        metadata: DiscoveryMetadata {
            region: region.into(),
            complete: true,
            requests: 0,
            quoted_at: now,
            failures: Vec::new(),
            notes,
            excluded_groups: None,
        },
    }
}

fn cover(topic: &str, scope: &str, status: &str, reason: &str) -> EstimateCoverage {
    EstimateCoverage {
        topic: topic.into(),
        scope: scope.into(),
        status: status.into(),
        reason: reason.into(),
    }
}

fn overflow(stage: &'static str, item_key: &str) -> Arc<Error> {
    Arc::new(
        Error::new(ErrorClass::EstimateOverflow).with_detail(EstimateArithmeticError {
            stage,
            item_key: item_key.into(),
        }),
    )
}

pub(super) fn account(
    item: &mut EstimateItem,
    quote: &Quote,
    notes: &mut Vec<PricingNote>,
) -> Shared<()> {
    let amount = quote.price.monthly_price.unwrap_or_default()
        * f64::from(u16::try_from(item.quantity).unwrap_or_default());
    if !amount.is_finite() {
        return Err(overflow("multiplication", &item.key));
    }
    item.price = quote.price.clone();
    item.monthly_amount = Some(amount);
    item.breakdown = quote
        .breakdown
        .iter()
        .map(|charge| EstimateCharge {
            name: charge.name.clone(),
            monthly_price: charge.monthly_price,
        })
        .collect();
    if quote.breakdown_mismatch {
        notes.push(note(
            "breakdown-mismatch",
            &item.key,
            "breakdown-mismatch",
            "",
            "observed",
        ));
    }
    Ok(())
}

impl Run<'_> {
    pub(super) fn record(&mut self, key: &str, error: Arc<Error>) {
        let api = error.api_error().filter(|api| !api.operation().is_empty());
        let mut code = api.map_or_else(|| "DiscoveryIncomplete".into(), failure_code);
        let reason = if let Some(arithmetic) = error.detail::<EstimateArithmeticError>() {
            code = if arithmetic.stage == "multiplication" {
                "EstimateMultiplicationOverflow"
            } else {
                "EstimateSubtotalOverflow"
            }
            .into();
            "arithmetic-overflow"
        } else {
            "request-failed"
        };
        self.output.metadata.complete = false;
        self.output.metadata.failures.push(DiscoveryFailure {
            operation: OP.into(),
            item_key: key.into(),
            code,
            status: i64::from(api.and_then(|api| api.status()).unwrap_or_default()),
        });
        self.output.coverage.push(cover(
            "processing",
            if key.is_empty() { "setup" } else { key },
            "failed",
            reason,
        ));
        self.errors.push(error);
    }

    pub(super) fn finish(&mut self) -> Shared<()> {
        let mut quoted = 0;
        let mut sum = 0.0;
        for item in &self.output.items {
            if let Some(amount) = item.monthly_amount {
                quoted += 1;
                sum += amount;
            }
        }
        if !sum.is_finite() {
            self.handle("", overflow("subtotal", ""))?;
        } else if quoted > 0 {
            self.output.total.known_monthly_subtotal = Some(sum);
        }
        self.output.total.requested_complete =
            self.output.metadata.complete && quoted == self.output.items.len();
        if self.output.total.requested_complete {
            self.output.total.requested_monthly_total = Some(sum);
        }
        for item in &self.output.items {
            if item.price.status == "failed"
                && !self
                    .output
                    .metadata
                    .failures
                    .iter()
                    .any(|failure| failure.item_key == item.key)
            {
                self.output.coverage.push(cover(
                    "processing",
                    &item.key,
                    "failed",
                    "not-attempted",
                ));
            }
        }
        self.output.metadata.requests = i64::try_from(self.catalog.requests()).unwrap_or_default();
        self.output.metadata.quoted_at = self.catalog.session.clock().utc_now();
        Ok(())
    }
}
