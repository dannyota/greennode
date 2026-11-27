use super::super::{
    Client, ResponseField,
    inventory::list_input,
    wire::{Family, failure},
};
use super::notification::{
    NotificationKind, QuotaUsageDetail, notification_kind, notification_reader, usage_descriptions,
};
use super::status::{Detail, Quota, decode_quota, notification_packages, read_synthetic_detail};
use super::wire::{Object, Reader, array, token};
use super::{Screen, output};
use crate::{Error, Failure, List, RequestContext};
use serde_json::value::RawValue;

list_input!(
    /// Selects overall Monitor quota usage.
    GetQuotaUsageInput
);
list_input!(
    /// Selects the current synthetic quota and its detail.
    GetSyntheticQuotaInput
);
list_input!(
    /// Selects consumed synthetic quota counters.
    GetSyntheticQuotaUsageInput
);

/// Selects the notification packages for an Email or SMS quota.
#[derive(Clone, Debug)]
pub struct ListNotificationPackagesInput {
    /// Email or SMS, matched case insensitively.
    pub kind: String,
}
impl ListNotificationPackagesInput {
    /// Selects the notification product whose packages are listed.
    pub fn new(kind: impl Into<String>) -> Self {
        Self { kind: kind.into() }
    }
}
/// Available packages for the selected notification product.
pub type ListNotificationPackagesOutput = List<NotificationPackage>;

output! {
    /// Account-wide Monitor quota usage.
    struct GetQuotaUsageOutput {
        /// Counts, free limits and severity per product.
        usage:QuotaUsage=>"Usage",
    }
}
output! {
    /// Modeled quota counters and severity across Monitor products.
    struct QuotaUsage {
        /// Synthetic usage rows.
        synthetic_usages:Vec<UsageCounts> =>"SyntheticUsages",
        /// Email usage rows.
        email_usages:Vec<NotificationUsage> =>"EmailUsages",
        /// Account's free email allowance.
        free_email_quota:ResponseField<i64> =>"FreeEmailQuota",
        /// Account's free SMS allowance.
        free_sms_quota:ResponseField<i64> =>"FreeSMSQuota",
        /// Log quota severity.
        log_severity:String=>"LogSeverity",
        /// Metric quota severity.
        metric_severity:String=>"MetricSeverity",
        /// Synthetic quota severity.
        synthetic_severity:String=>"SyntheticSeverity",
        /// Email quota severity.
        email_severity:String=>"EmailSeverity",
        /// SMS quota severity.
        sms_severity:String=>"SMSSeverity",
        /// Notification quota severity.
        notification_severity:String=>"NotificationSeverity",
        /// Kinds with usage entries whose shape is unknown.
        unmodeled_kinds:Vec<String> =>"UnmodeledKinds",
    }
}
output! {
    /// Synthetic usage counts.
    struct UsageCounts {
        /// API checks.
        api:ResponseField<i64> =>"API",
        /// Locations.
        location:ResponseField<i64> =>"Location",
        /// Email notifications.
        email:ResponseField<i64> =>"Email",
        /// SMS notifications.
        sms:ResponseField<i64> =>"SMS",
        /// Slack notifications.
        slack:ResponseField<i64> =>"Slack",
        /// Webhook notifications.
        webhook:ResponseField<i64> =>"Webhook",
        /// Telegram notifications.
        telegram:ResponseField<i64> =>"Telegram",
        /// Teams notifications.
        teams:ResponseField<i64> =>"Teams",
    }
}
output! {
    /// Notification consumption.
    struct NotificationUsage {
        /// Number of notifications counted.
        amount:ResponseField<i64> =>"Amount",
    }
}
output! {
    /// A synthetic quota and its pricing detail.
    struct GetSyntheticQuotaOutput {
        /// Current account quota.
        quota:Quota=>"Quota",
        /// Limits and price for the quota.
        detail:SyntheticQuotaDetail=>"Detail",
    }
}
output! {
    /// Usage and descriptions for the synthetic quota.
    struct GetSyntheticQuotaUsageOutput {
        /// Usage counters and their meanings.
        usage:QuotaUsageDetail=>"Usage",
    }
}
output! {
    /// Optional limits for each synthetic notification channel.
    struct SyntheticNotificationLimits {
        /// Email limit.
        email:Option<ResponseField<i64>> =>"Email",
        /// SMS limit.
        sms:Option<ResponseField<i64>> =>"SMS",
        /// Slack limit.
        slack:Option<ResponseField<i64>> =>"Slack",
        /// Webhook limit.
        webhook:Option<ResponseField<i64>> =>"Webhook",
        /// Telegram limit.
        telegram:Option<ResponseField<i64>> =>"Telegram",
        /// Teams limit.
        teams:Option<ResponseField<i64>> =>"Teams",
    }
}
output! {
    /// Synthetic quota detail and optional list price.
    struct SyntheticQuotaDetail {
        /// Channel limits, retaining unknown values.
        channel_limits:SyntheticNotificationLimits=>"ChannelLimits",
        /// Allocated API checks.
        api:ResponseField<i64> =>"API",
        /// Allocated locations.
        location:ResponseField<i64> =>"Location",
        /// Email limit, or zero if unknown.
        email:ResponseField<i64> =>"Email",
        /// SMS limit, or zero if unknown.
        sms:ResponseField<i64> =>"SMS",
        /// Slack limit, or zero if unknown.
        slack:ResponseField<i64> =>"Slack",
        /// Webhook limit, or zero if unknown.
        webhook:ResponseField<i64> =>"Webhook",
        /// Telegram limit, or zero if unknown.
        telegram:ResponseField<i64> =>"Telegram",
        /// Teams limit, or zero if unknown.
        teams:ResponseField<i64> =>"Teams",
        /// Quota class name.
        class_name:String=>"ClassName",
        /// Quota tier name.
        tier_name:String=>"TierName",
        /// Price in VND when reported.
        price:Option<ResponseField<f64>> =>"Price",
    }
}
output! {
    /// A package for notification quota.
    struct NotificationPackage {
        /// Package identifier.
        id:String=>"ID",
        /// Package name.
        name:String=>"Name",
        /// Package price in VND when reported.
        price:Option<ResponseField<f64>> =>"Price",
        /// Number of notifications included.
        amount:ResponseField<i64> =>"Amount",
    }
}

impl Client {
    /// Reads account-wide Monitor quota usage.
    pub async fn get_quota_usage(
        &self,
        ctx: &RequestContext,
        _: GetQuotaUsageInput,
    ) -> Result<GetQuotaUsageOutput, Failure<GetQuotaUsageOutput>> {
        let mut reader = redirecting(self, ctx, "monitor.GetQuotaUsage");
        let raw = reader
            .get("billing-api/v1/quotas/usages", &[])
            .await
            .map_err(failure)?;
        let usage = reader.decode(aggregate(&raw)).map_err(failure)?;
        screened(GetQuotaUsageOutput { usage }, &reader).map_err(failure)
    }
    /// Reads the current synthetic quota and its detail.
    pub async fn get_synthetic_quota(
        &self,
        ctx: &RequestContext,
        _: GetSyntheticQuotaInput,
    ) -> Result<GetSyntheticQuotaOutput, Failure<GetSyntheticQuotaOutput>> {
        let mut reader = redirecting(self, ctx, "monitor.GetSyntheticQuota");
        let raw = reader
            .get("billing-api/v1/synthetic/quota", &[])
            .await
            .map_err(failure)?;
        let quota = reader.decode(decode_quota(&raw)).map_err(failure)?;
        let detail = read_synthetic_detail(&quota, &mut reader)
            .await
            .map_err(failure)?;
        screened(
            GetSyntheticQuotaOutput {
                quota: quota.into_model(),
                detail: detail.into(),
            },
            &reader,
        )
        .map_err(failure)
    }
    /// Reads usage and provider descriptions for synthetic quota.
    pub async fn get_synthetic_quota_usage(
        &self,
        ctx: &RequestContext,
        _: GetSyntheticQuotaUsageInput,
    ) -> Result<GetSyntheticQuotaUsageOutput, Failure<GetSyntheticQuotaUsageOutput>> {
        let mut reader = redirecting(self, ctx, "monitor.GetSyntheticQuotaUsage");
        let raw = reader
            .get("billing-api/v1/synthetic/quota/usages", &[])
            .await
            .map_err(failure)?;
        let usage = reader.decode(synthetic_usage(&raw)).map_err(failure)?;
        screened(GetSyntheticQuotaUsageOutput { usage }, &reader).map_err(failure)
    }
    /// Lists notification package prices without ordering them.
    pub async fn list_notification_packages(
        &self,
        ctx: &RequestContext,
        input: ListNotificationPackagesInput,
    ) -> Result<ListNotificationPackagesOutput, Failure<ListNotificationPackagesOutput>> {
        const OP: &str = "monitor.ListNotificationPackages";
        let kind = notification_kind(OP, &input.kind).map_err(failure)?;
        let mut reader = notification_reader(self, ctx, OP);
        let route = match kind {
            NotificationKind::Email => "billing-api/v1/email/packages",
            NotificationKind::Sms => "billing-api/v1/sms/packages",
        };
        let raw = reader.get(route, &[]).await.map_err(failure)?;
        let values = reader
            .decode(notification_packages(&raw))
            .map_err(failure)?;
        let mut items: Vec<_> = values
            .into_iter()
            .map(|package| NotificationPackage {
                id: package.id,
                name: package.name,
                price: package.price.map(ResponseField::Value),
                amount: ResponseField::Value(package.amount),
            })
            .collect();
        if items
            .iter()
            .any(|package| reader.evidence.id_reflects(&package.id))
            || !items.screen(&reader.evidence)
        {
            return Err(failure(reader.invalid()));
        }
        Ok(List { items })
    }
}
fn redirecting<'a>(client: &'a Client, ctx: &'a RequestContext, op: &'static str) -> Reader<'a> {
    let mut reader = Reader::new(client, ctx, op);
    reader.read.family = Family::Quota { no_redirect: false };
    reader
}
fn screened<T: Screen>(mut value: T, reader: &Reader<'_>) -> Result<T, Error> {
    if !value.screen(&reader.evidence) {
        return Err(reader.invalid());
    }
    Ok(value)
}
fn aggregate(raw: &RawValue) -> Option<QuotaUsage> {
    let o = Object::of(raw)?;
    let text = |name| token::<Option<String>>(o.required(name)?).map(Option::unwrap_or_default);
    let mut unmodeled_kinds = Vec::new();
    for (kind, key) in [
        ("log", "logUsages"),
        ("metric", "metricUsages"),
        ("sms", "smsUsages"),
    ] {
        if !array(o.required(key)?)?.is_empty() {
            unmodeled_kinds.push(kind.into());
        }
    }
    Some(QuotaUsage {
        synthetic_usages: array(o.required("syntheticUsages")?)?
            .into_iter()
            .map(counts)
            .collect::<Option<_>>()?,
        email_usages: array(o.required("emailUsages")?)?
            .into_iter()
            .map(|raw| {
                Some(NotificationUsage {
                    amount: ResponseField::Value(Object::of(raw)?.integer("amount")?),
                })
            })
            .collect::<Option<_>>()?,
        free_email_quota: ResponseField::Value(o.integer("freeEmailQuota")?),
        free_sms_quota: ResponseField::Value(o.integer("freeSmsQuota")?),
        log_severity: text("logSeverity")?,
        metric_severity: text("metricSeverity")?,
        synthetic_severity: text("syntheticSeverity")?,
        email_severity: text("emailSeverity")?,
        sms_severity: text("smsSeverity")?,
        notification_severity: text("notificationSeverity")?,
        unmodeled_kinds,
    })
}
fn counts(raw: &RawValue) -> Option<UsageCounts> {
    let o = Object::of(raw)?;
    Some(UsageCounts {
        api: ResponseField::Value(o.integer("api")?),
        location: ResponseField::Value(o.integer("location")?),
        email: ResponseField::Value(o.integer("email")?),
        sms: ResponseField::Value(o.integer("sms")?),
        slack: ResponseField::Value(o.integer("slack")?),
        webhook: ResponseField::Value(o.integer("webhook")?),
        telegram: ResponseField::Value(o.integer("telegram")?),
        teams: ResponseField::Value(o.integer("teams")?),
    })
}

fn synthetic_usage(raw: &RawValue) -> Option<QuotaUsageDetail> {
    let o = Object::of(raw)?;
    let usage = counts(o.required("usage")?)?;
    Some(QuotaUsageDetail {
        counts: Some(usage),
        amount: ResponseField::Value(0),
        descriptions: usage_descriptions(&o)?,
    })
}
impl From<Detail> for SyntheticQuotaDetail {
    fn from(detail: Detail) -> Self {
        let [email, sms, slack, webhook, telegram, teams] = detail.limits;
        let optional = |value: Option<i64>| value.map(ResponseField::Value);
        let default = |value: Option<i64>| ResponseField::Value(value.unwrap_or(0));
        Self {
            channel_limits: SyntheticNotificationLimits {
                email: optional(email),
                sms: optional(sms),
                slack: optional(slack),
                webhook: optional(webhook),
                telegram: optional(telegram),
                teams: optional(teams),
            },
            api: ResponseField::Value(detail.api),
            location: ResponseField::Value(detail.location),
            email: default(email),
            sms: default(sms),
            slack: default(slack),
            webhook: default(webhook),
            telegram: default(telegram),
            teams: default(teams),
            class_name: detail.class_name,
            tier_name: detail.tier_name,
            price: detail.price.map(ResponseField::Value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic_usage;
    use serde_json::value::RawValue;

    #[test]
    fn zero_synthetic_usage_keeps_all_counts() {
        let raw = RawValue::from_string(r#"{"usage":{"api":0,"location":0,"email":0,"sms":0,"slack":0,"webhook":0,"telegram":0,"teams":0},"description":{}}"#.to_owned()).unwrap();
        let value = synthetic_usage(&raw).unwrap();
        let output = serde_json::to_value(value).unwrap();
        assert_eq!(output["Counts"].as_object().unwrap().len(), 8);
    }
}
