use std::collections::BTreeMap;

use serde::Serializer;
use serde_json::value::RawValue;

use super::super::{
    Client, ResponseField,
    wire::{Family, failure},
};
use super::quotes::usage;
use super::status::{Quota, decode_quota};
use super::wire::{Object, Reader};
use super::{Screen, output};
use crate::wire::product::evidence::Evidence;
use crate::{Error, Failure, RequestContext};

macro_rules! kind_input {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        pub struct $name {
            /// Notification quota kind, matched case insensitively.
            pub kind: String,
        }
        impl $name {
            /// Selects the notification quota kind.
            pub fn new(kind: impl Into<String>) -> Self {
                Self { kind: kind.into() }
            }
        }
    };
}

kind_input!(
    /// Selects the Email or SMS quota to read.
    GetNotificationQuotaInput
);
kind_input!(
    /// Selects the Email quota usage to read.
    GetNotificationQuotaUsageInput
);

output! {
    /// The selected notification quota.
    struct GetNotificationQuotaOutput {
        /// Quota returned by the provider.
        quota: Quota => "Quota",
    }
}

output! {
    /// The selected notification quota usage.
    struct GetNotificationQuotaUsageOutput {
        /// Number of messages used and provider descriptions.
        usage: QuotaUsageDetail => "Usage",
    }
}

output! {
    /// Detailed usage for a Monitor quota.
    struct QuotaUsageDetail {
        /// Amount of quota used.
        amount: ResponseField<i64> => "Amount",
        /// Provider descriptions keyed by field.
        #[serde(serialize_with = "serialize_descriptions")]
        descriptions: BTreeMap<String, UsageDescription> => "Descriptions",
    }
}

output! {
    /// Describes a usage field returned by the provider.
    struct UsageDescription {
        /// Display name.
        name: String => "Name",
        /// Description of what the field measures.
        specs: String => "Specs",
        /// Unit of measurement.
        unit: String => "Unit",
    }
}

impl Client {
    /// Reads the account's Email or SMS quota.
    pub async fn get_notification_quota(
        &self,
        ctx: &RequestContext,
        input: GetNotificationQuotaInput,
    ) -> Result<GetNotificationQuotaOutput, Failure<GetNotificationQuotaOutput>> {
        const OP: &str = "monitor.GetNotificationQuota";
        let kind = notification_kind(OP, &input.kind).map_err(failure)?;
        let mut reader = notification_reader(self, ctx, OP);
        let route = kind.quota_route();
        let raw = reader.get(route, &[]).await.map_err(failure)?;
        let quota = reader
            .decode(decode_quota(&raw))
            .map_err(failure)?
            .into_model();
        if [&quota.id, &quota.package_id]
            .into_iter()
            .any(|id| reader.evidence.id_reflects(id))
        {
            return Err(failure(reader.invalid()));
        }
        let mut output = GetNotificationQuotaOutput { quota };
        if !output.screen(&reader.evidence) {
            return Err(failure(reader.invalid()));
        }
        Ok(output)
    }

    /// Reads the number of messages charged against the Email notification quota.
    pub async fn get_notification_quota_usage(
        &self,
        ctx: &RequestContext,
        input: GetNotificationQuotaUsageInput,
    ) -> Result<GetNotificationQuotaUsageOutput, Failure<GetNotificationQuotaUsageOutput>> {
        const OP: &str = "monitor.GetNotificationQuotaUsage";
        if notification_kind(OP, &input.kind).map_err(failure)? == NotificationKind::Sms {
            return Err(failure(usage(OP, "SMS usage read is unsupported")));
        }
        let mut reader = notification_reader(self, ctx, OP);
        let raw = reader
            .get("billing-api/v1/email/quota/usages", &[])
            .await
            .map_err(failure)?;
        let usage = reader.decode(notification_usage(&raw)).map_err(failure)?;
        let mut output = GetNotificationQuotaUsageOutput { usage };
        if !output.screen(&reader.evidence) {
            return Err(failure(reader.invalid()));
        }
        Ok(output)
    }
}

pub(super) fn notification_reader<'a>(
    client: &'a Client,
    ctx: &'a RequestContext,
    operation: &'static str,
) -> Reader<'a> {
    let mut reader = Reader::new(client, ctx, operation);
    reader.read.family = Family::Quota { no_redirect: false };
    reader
}

#[derive(PartialEq, Eq)]
pub(super) enum NotificationKind {
    Email,
    Sms,
}

impl NotificationKind {
    pub(super) fn quota_route(&self) -> &'static str {
        match self {
            Self::Email => "billing-api/v1/email/quota",
            Self::Sms => "billing-api/v1/sms/quota",
        }
    }
}

pub(super) fn notification_kind(op: &'static str, kind: &str) -> Result<NotificationKind, Error> {
    if kind.is_empty() {
        return Err(super::super::wire::input_error(format!(
            "vngcloud: invalid input: {op} requires Kind"
        )));
    }
    if kind.eq_ignore_ascii_case("email") {
        return Ok(NotificationKind::Email);
    }
    if kind.eq_ignore_ascii_case("sms") {
        return Ok(NotificationKind::Sms);
    }
    Err(usage(op, "Kind must be Email or SMS"))
}

fn notification_usage(raw: &RawValue) -> Option<QuotaUsageDetail> {
    let object = Object::of(raw)?;
    let usage = Object::of(object.required("usage")?)?;
    let descriptions = Object::of(object.required("description")?)?
        .entries()
        .map(|(key, raw)| {
            let item = Object::of(raw)?;
            let description = UsageDescription {
                name: item.text("name")?,
                specs: item.text("specs")?,
                unit: item.text("unit")?,
            };
            Some((key, description))
        })
        .collect::<Option<_>>()?;
    Some(QuotaUsageDetail {
        amount: ResponseField::Value(usage.integer("amount")?),
        descriptions,
    })
}

impl Screen for BTreeMap<String, UsageDescription> {
    fn screen(&mut self, evidence: &Evidence) -> bool {
        self.retain(|key, _| !evidence.text_reflects(key));
        self.values_mut()
            .all(|description| description.screen(evidence))
    }
}

fn serialize_descriptions<S: Serializer>(
    descriptions: &BTreeMap<String, UsageDescription>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_newtype_struct("$greennode::dynamic-map", descriptions)
}
