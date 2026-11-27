use std::collections::HashSet;

use http::StatusCode;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::{
    Client, ResponseField,
    wire::{Family, failure},
};
use super::catalog::{ACTIVE, OfferIssue, QuoteOffer, quote_offer};
use super::notification::{NotificationKind, notification_kind, notification_reader};
use super::status::{
    ErrorDetail, QuotaTimestamp, decode_quota, notification_packages, read_synthetic_detail,
};
use super::wire::{Object, Reader};
use super::{Screen, output};
use crate::quote::request::{self, Product, Request};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext};

const CREATE: &str = "monitor.QuoteCreateSyntheticQuota";
const RENEW: &str = "monitor.QuoteRenewSyntheticQuota";
const SOURCE_QUOTA: &str = "billing-api/v1/synthetic/quota";
const PRICE_ROUTE: &str = "billing-api/v2/synthetic/prices/created-price";

/// Selects the class, package and quantity for a synthetic quota quote.
#[derive(Clone, Debug)]
pub struct CreateSyntheticQuotaInput {
    /// Class whose unit package will be priced.
    pub class_id: String,
    /// Package within that class.
    pub package_id: String,
    /// Number of API checks to price.
    pub quantity: i64,
    /// Cost limit for an order. The quote ignores it.
    pub max_price: f64,
    /// Settlement preference for an order. The quote ignores it.
    pub no_wait: bool,
}

impl CreateSyntheticQuotaInput {
    /// Selects a class, package and quantity to price.
    pub fn new(class_id: impl Into<String>, package_id: impl Into<String>, quantity: i64) -> Self {
        Self {
            class_id: class_id.into(),
            package_id: package_id.into(),
            quantity,
            max_price: 0.0,
            no_wait: false,
        }
    }
}

/// Selects the active quota expiry for a renewal quote.
#[derive(Clone, Debug)]
pub struct RenewSyntheticQuotaInput {
    /// Expected RFC 3339 expiry of the active quota.
    pub expected_end_time: String,
    /// Cost limit for renewal. The quote ignores it.
    pub max_price: f64,
    /// When to skip a renewal. The quote ignores it.
    pub max_days_left: i64,
    /// Settlement preference for renewal. The quote ignores it.
    pub no_wait: bool,
}

impl RenewSyntheticQuotaInput {
    /// Selects the quota expiry to check before pricing.
    pub fn new(expected_end_time: impl Into<String>) -> Self {
        Self {
            expected_end_time: expected_end_time.into(),
            max_price: 0.0,
            max_days_left: 0,
            no_wait: false,
        }
    }
}

output! {
    /// Price for one month of synthetic quota.
    struct QuotaQuote {
        /// Selected class package identifier.
        package_id: String => "PackageID",
        /// Quoted number of API checks.
        quantity: ResponseField<i64> => "Quantity",
        /// Length of the quoted term in months.
        month_period: ResponseField<i64> => "MonthPeriod",
        /// Existing quota identifier for renewal.
        resource_id: String => "ResourceID",
        /// Existing quota expiry for renewal, with its reported offset.
        expected_end_time: Option<ResponseField<QuotaTimestamp>> => "ExpectedEndTime",
        /// Price from the quote endpoint, in VND.
        optimum_price: ResponseField<f64> => "OptimumPrice",
        /// Price inferred from the matching catalog and detail, in VND.
        list_price: ResponseField<f64> => "ListPrice",
        /// Higher of the quoted and inferred prices, in VND.
        guard_price: ResponseField<f64> => "GuardPrice",
        /// Currency of the quote.
        currency: String => "Currency",
    }
}

/// Selects an Email or SMS package for a notification quota quote.
#[derive(Clone, Debug)]
pub struct CreateNotificationQuotaInput {
    /// Email or SMS, matched case insensitively.
    pub kind: String,
    /// Package identifier. Empty selects the unique free package.
    pub package_id: String,
    /// Cost limit for an order. The quote ignores it.
    pub max_price: f64,
    /// Settlement preference for an order. The quote ignores it.
    pub no_wait: bool,
}

impl CreateNotificationQuotaInput {
    /// Selects the notification quota kind to price.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            package_id: String::new(),
            max_price: 0.0,
            no_wait: false,
        }
    }
}

/// A quote for a new notification quota.
pub type QuoteCreateNotificationQuotaOutput = QuotaQuote;

/// A quote for a new synthetic quota.
pub type QuoteCreateSyntheticQuotaOutput = QuotaQuote;
/// A quote for renewing the current synthetic quota.
pub type QuoteRenewSyntheticQuotaOutput = QuotaQuote;

impl Client {
    /// Prices one month of Email or SMS quota after verifying absence.
    pub async fn quote_create_notification_quota(
        &self,
        ctx: &RequestContext,
        input: CreateNotificationQuotaInput,
    ) -> Result<QuoteCreateNotificationQuotaOutput, Failure<QuoteCreateNotificationQuotaOutput>>
    {
        self.notification_quote(ctx, input).await.map_err(failure)
    }

    async fn notification_quote(
        &self,
        ctx: &RequestContext,
        input: CreateNotificationQuotaInput,
    ) -> Result<QuotaQuote, Error> {
        const OP: &str = "monitor.QuoteCreateNotificationQuota";
        let kind = notification_kind(OP, &input.kind)?;
        let mut reader = notification_reader(self, ctx, OP);
        self.validate_quote_inputs(ctx, &mut reader.evidence, &[&input.package_id])?;
        let (package_route, price_route, absent) = match kind {
            NotificationKind::Email => (
                "billing-api/v1/email/packages",
                "billing-api/v2/email/prices/created-price",
                "Not found email quota",
            ),
            NotificationKind::Sms => (
                "billing-api/v1/sms/packages",
                "billing-api/v2/sms/prices/created-price",
                "Not found sms quota",
            ),
        };
        require_absent(
            &mut reader,
            kind.quota_route(),
            absent,
            "a notification quota already exists",
        )
        .await?;
        let raw = reader.get(package_route, &[]).await?;
        let packages = reader.decode(notification_packages(&raw))?;
        let mut seen = HashSet::new();
        let mut selected = None;
        for package in &packages {
            if reader.evidence.id_reflects(&package.id) {
                return Err(reader.invalid());
            }
            if !seen.insert(&package.id) {
                return Err(usage(OP, "duplicate package IDs"));
            }
            let matches = if input.package_id.is_empty() {
                package.price == Some(0.0)
            } else {
                package.id == input.package_id
            };
            if matches && selected.replace(package).is_some() {
                return Err(usage(OP, "ambiguous free package selection"));
            }
        }
        let package = selected.ok_or_else(|| usage(OP, "no matching package"))?;
        let list_price = reader.decode(package.price)?;
        let body = json!({"redirectUrl": "", "packageId": package.id, "quantity": 1});
        finish_quote(
            self,
            ctx,
            &mut reader,
            Terms {
                class_id: None,
                package_id: &package.id,
                quantity: 1,
                list_price,
                renewal: None,
                price_route,
                body,
            },
        )
        .await
    }

    /// Quotes a one-month synthetic quota after checking absence and catalog eligibility.
    pub async fn quote_create_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: CreateSyntheticQuotaInput,
    ) -> Result<QuoteCreateSyntheticQuotaOutput, Failure<QuoteCreateSyntheticQuotaOutput>> {
        self.create_synthetic_quote(ctx, input)
            .await
            .map_err(failure)
    }

    /// Quotes one month of renewal for the active synthetic quota at the expected expiry.
    pub async fn quote_renew_synthetic_quota(
        &self,
        ctx: &RequestContext,
        input: RenewSyntheticQuotaInput,
    ) -> Result<QuoteRenewSyntheticQuotaOutput, Failure<QuoteRenewSyntheticQuotaOutput>> {
        self.renew_synthetic_quote(ctx, input)
            .await
            .map_err(failure)
    }

    async fn create_synthetic_quote(
        &self,
        ctx: &RequestContext,
        input: CreateSyntheticQuotaInput,
    ) -> Result<QuotaQuote, Error> {
        for (field, empty) in [
            ("ClassID", input.class_id.is_empty()),
            ("PackageID", input.package_id.is_empty()),
            ("Quantity", input.quantity == 0),
        ] {
            if empty {
                return Err(
                    Error::new(ErrorClass::InvalidInput).with_outer_detail(format!(
                        "vngcloud: invalid input: {CREATE} requires {field}"
                    )),
                );
            }
        }
        if !PathIds::Strict.allows(&input.class_id) {
            return Err(
                Error::new(ErrorClass::InvalidInput).with_outer_detail(format!(
                    "vngcloud: invalid input: {CREATE} requires ClassID to match ^[A-Za-z0-9-]+$"
                )),
            );
        }
        if input.quantity <= 0 {
            return Err(usage(CREATE, "Quantity must be positive"));
        }
        let mut reader = Reader::new(self, ctx, CREATE);
        let identifiers = [input.class_id.as_str(), input.package_id.as_str()];
        self.validate_quote_inputs(ctx, &mut reader.evidence, &identifiers)?;
        require_absent(
            &mut reader,
            SOURCE_QUOTA,
            "Not found synthetic quota",
            "synthetic quota already exists",
        )
        .await?;
        let offer = resolve_offer(
            &mut reader,
            &input.class_id,
            &input.package_id,
            input.quantity,
        )
        .await?;
        let body = json!({"redirectUrl":"", "packageId": input.package_id, "quantity": input.quantity, "buyWith": {}});
        finish_quote(
            self,
            ctx,
            &mut reader,
            Terms {
                class_id: Some(&input.class_id),
                price_route: PRICE_ROUTE,
                package_id: &input.package_id,
                quantity: input.quantity,
                renewal: None,
                list_price: offer.list_price,
                body,
            },
        )
        .await
    }

    async fn renew_synthetic_quote(
        &self,
        ctx: &RequestContext,
        input: RenewSyntheticQuotaInput,
    ) -> Result<QuotaQuote, Error> {
        let expiry = OffsetDateTime::parse(&input.expected_end_time, &Rfc3339)
            .ok()
            .filter(|time| *time != QuotaTimestamp::default().0)
            .ok_or_else(|| usage(RENEW, "ExpectedEndTime must be RFC 3339"))?;
        let mut reader = Reader::new(self, ctx, RENEW);
        reader.read.family = Family::Quota { no_redirect: false };
        self.validate_quote_inputs(ctx, &mut reader.evidence, &[&input.expected_end_time])?;
        let raw = reader.get(SOURCE_QUOTA, &[]).await?;
        let quota = reader.decode(decode_quota(&raw))?;
        let detail = read_synthetic_detail(&quota, &mut reader).await?;
        if quota.end_time.0 != expiry {
            return Err(Error::new(ErrorClass::Api).with_outer_detail(ErrorDetail::QuotaChanged));
        }
        match quota.id_fields.status.as_str() {
            ACTIVE => (),
            "IN-TRASH" | "CREATING" | "ERROR" => return Err(usage(RENEW, "quota is ineligible")),
            _ => {
                return Err(
                    Error::new(ErrorClass::Api).with_outer_detail(ErrorDetail::UnexpectedStatus)
                );
            }
        }
        if quota.poc
            || quota.auto_renew
            || quota.start_time == QuotaTimestamp::default()
            || quota.end_time.0 <= self.session.clock().utc_now()
        {
            return Err(usage(RENEW, "quota is ineligible"));
        }
        let price = reader.decode(detail.price)?;
        let ids = &quota.id_fields;
        reader.read.family = Family::Quota { no_redirect: true };
        let offer =
            resolve_offer(&mut reader, &detail.class_id, &ids.package_id, detail.api).await?;
        if offer.location != detail.location {
            return Err(usage(RENEW, "synthetic location is unsupported"));
        }
        let body = json!({"redirectUrl":"", "packageId": ids.package_id, "quantity": detail.api, "resourceId": ids.id});
        finish_quote(
            self,
            ctx,
            &mut reader,
            Terms {
                class_id: Some(&detail.class_id),
                price_route: PRICE_ROUTE,
                package_id: &ids.package_id,
                quantity: detail.api,
                renewal: Some((&ids.id, quota.end_time)),
                list_price: offer.list_price.max(price),
                body,
            },
        )
        .await
    }

    fn validate_quote_inputs(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        identifiers: &[&str],
    ) -> Result<(), Error> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if identifiers.iter().any(|id| evidence.id_reflects(id)) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        Ok(())
    }
}

struct Terms<'a> {
    class_id: Option<&'a str>,
    price_route: &'static str,
    package_id: &'a str,
    quantity: i64,
    list_price: f64,
    renewal: Option<(&'a str, QuotaTimestamp)>,
    body: Value,
}

async fn resolve_offer(
    reader: &mut Reader<'_>,
    class_id: &str,
    package_id: &str,
    quantity: i64,
) -> Result<QuoteOffer, Error> {
    quote_offer(reader, class_id, package_id, quantity)
        .await?
        .map_err(|issue| {
            usage(
                reader.read.id,
                match issue {
                    OfferIssue::Class => "selected synthetic class is unsupported",
                    OfferIssue::Package => "selected synthetic package is unsupported",
                    OfferIssue::Option => "synthetic quantity or package option is unsupported",
                    OfferIssue::Period => "synthetic month period is unsupported",
                },
            )
        })
}

async fn finish_quote(
    client: &Client,
    ctx: &RequestContext,
    reader: &mut Reader<'_>,
    terms: Terms<'_>,
) -> Result<QuotaQuote, Error> {
    let resource_id = terms.renewal.map_or("", |(id, _)| id);
    let ids = [
        terms.class_id.unwrap_or_default(),
        terms.package_id,
        resource_id,
    ];
    if ids.iter().any(|id| reader.evidence.id_reflects(id)) {
        return Err(reader.invalid());
    }
    let op = reader.read.id;
    let raw = request::exchange(
        &client.session,
        ctx,
        &mut reader.evidence,
        Request {
            operation: op,
            template: terms.price_route,
            product: Product::Monitor {
                identifiers: &ids,
                no_redirect: terms.class_id.is_some() && terms.renewal.is_none(),
            },
            path: &[],
            query: &[],
            body: Some(&terms.body),
        },
        None,
    )
    .await
    .map_err(|error| *error)?;
    if ids.iter().any(|id| reader.evidence.id_reflects(id))
        || reader.evidence.credentials().values().len() > 256
    {
        return Err(reader.invalid());
    }
    let optimum = reader.decode(request::monitor::price(&raw))?;
    if terms.list_price > 0.0 && optimum == 0.0 {
        return Err(Error::new(ErrorClass::Unpriced).with_outer_detail(format!("{op}: vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource")));
    }
    let guard = terms.list_price.max(optimum);
    if terms.class_id.is_some() && guard != 0.0 {
        return Err(usage(op, "paid synthetic quota is unsupported"));
    }
    let mut output = QuotaQuote {
        package_id: terms.package_id.into(),
        quantity: ResponseField::Value(terms.quantity),
        month_period: ResponseField::Value(1),
        resource_id: resource_id.into(),
        expected_end_time: terms.renewal.map(|(_, end)| ResponseField::Value(end)),
        optimum_price: ResponseField::Value(optimum),
        list_price: ResponseField::Value(terms.list_price),
        guard_price: ResponseField::Value(guard),
        currency: "VND".into(),
    };
    if !output.screen(&reader.evidence) {
        return Err(reader.invalid());
    }
    Ok(output)
}

async fn require_absent(
    reader: &mut Reader<'_>,
    route: &'static str,
    expected: &str,
    exists: &str,
) -> Result<(), Error> {
    let (status, raw) = reader.get_status(route, &[], true).await?;
    if status == StatusCode::OK {
        reader.decode(decode_quota(&raw))?;
        return Err(usage(reader.read.id, exists));
    }
    if Object::of(&raw).is_some_and(|body| {
        body.len() == 1
            && body.text("message").is_some_and(|message| {
                message == expected && !reader.evidence.text_reflects(&message)
            })
    }) {
        return Ok(());
    }
    Err(reader.read.error(
        Some(404),
        "NotFound",
        "quota response withheld",
        false,
        &reader.evidence,
    ))
}

pub(super) fn usage(operation: &str, reason: &str) -> Error {
    Error::new(ErrorClass::InvalidInput)
        .with_outer_detail(format!("{operation}: {reason}: vngcloud: invalid input"))
}
