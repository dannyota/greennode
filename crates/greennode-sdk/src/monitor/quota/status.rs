use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use super::super::{Client, ResponseField, wire::failure};
use super::catalog::{
    ACTIVE, read_class_packages, read_classes, read_settings, status_eligibility,
};
use super::wire::{Object, Reader, array, integer, token};
use super::{Screen, output, unique};
use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

const OPERATION: &str = "monitor.GetQuotaStatus";

/// Service-specific Monitor failure identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorDetail {
    /// At least one row of the quota status snapshot was unavailable.
    QuotaStatusIncomplete,
    /// The requested quota kind is unsupported.
    InvalidQuotaKind,
    /// The current quota expiry differs from the expected expiry.
    QuotaChanged,
    /// The provider returned a status without known quota semantics.
    UnexpectedStatus,
}

/// Selects which quotas to include in a status snapshot.
#[derive(Clone, Debug, Default)]
pub struct GetQuotaStatusInput {
    /// Optional kind, matched case insensitively. Empty selects all quotas.
    pub kind: String,
}

/// A timestamp returned by a Monitor quota read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaTimestamp(pub OffsetDateTime);

impl QuotaTimestamp {
    /// Returns the same instant with a UTC offset, if representable.
    pub fn to_utc(self) -> Option<Self> {
        self.0.checked_to_offset(UtcOffset::UTC).map(Self)
    }
}

impl Default for QuotaTimestamp {
    fn default() -> Self {
        Self(
            OffsetDateTime::from_unix_timestamp(-62_135_596_800)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        )
    }
}

impl Serialize for QuotaTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for QuotaTimestamp {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        let text = String::deserialize(input)?;
        OffsetDateTime::parse(&text, &Rfc3339)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

impl WireField for QuotaTimestamp {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.format(&Rfc3339).is_ok_and(|text| {
            let display = self
                .0
                .checked_to_offset(UtcOffset::UTC)
                .and_then(|time| time.format(&Rfc3339).ok());
            !evidence.text_reflects(&text)
                && display.is_some_and(|text| {
                    let text = text.trim_end_matches('Z').replace('T', " ") + " UTC";
                    !evidence.text_reflects(&text)
                })
        })
    }
}

output! {
    /// A sequential snapshot of the requested Monitor quotas.
    struct GetQuotaStatusOutput {
        /// UTC time used for every row's timing calculation.
        as_of: ResponseField<QuotaTimestamp> => "AsOf",
        /// Quota rows in Synthetic, Email, SMS order, as requested.
        items: Vec<QuotaStatusRow> => "Items",
        /// Whether every requested row was available.
        complete: ResponseField<bool> => "Complete",
    }
}

output! {
    /// One quota and its known status, current catalog price and timing.
    struct QuotaStatusRow {
        /// Quota kind.
        kind: String => "Kind",
        /// Synthetic class name, when known.
        class_name: String => "ClassName",
        /// Notification package name, when known.
        package_name: String => "PackageName",
        /// Provider status, none, or unavailable.
        status: String => "Status",
        /// Quota returned by the provider.
        quota: Option<Quota> => "Quota",
        /// Current list price in VND, if known.
        price: Option<ResponseField<f64>> => "Price",
        /// UTC start time, if known.
        start_time: Option<ResponseField<QuotaTimestamp>> => "StartTime",
        /// UTC end time, if known.
        end_time: Option<ResponseField<QuotaTimestamp>> => "EndTime",
        /// Whole seconds until expiration, negative after expiration.
        time_left_seconds: Option<ResponseField<i64>> => "TimeLeftSeconds",
        /// Configured auto-renew setting, if known.
        auto_renew: Option<ResponseField<bool>> => "AutoRenew",
        /// Whether an evidenced free renewal window is open, if known.
        renew_window_open: Option<ResponseField<bool>> => "RenewWindowOpen",
        /// Command suggested by the snapshot.
        next_safe_command: String => "NextSafeCommand",
        /// Safe provider error metadata, if the row failed.
        error: Option<QuotaStatusError> => "Error",
    }
}

output! {
    /// Safe failure metadata for an unavailable quota status row.
    struct QuotaStatusError {
        /// Service or fallback error code.
        code: String => "Code",
        /// SDK operation used for the snapshot.
        operation: String => "Operation",
        /// HTTP status, or zero when there was no response.
        status: ResponseField<i64> => "Status",
    }
}

output! {
    /// A Monitor quota returned by the billing API.
    struct Quota {
        /// Quota identifier.
        id: String => "ID",
        /// Provider quota name.
        name: String => "Name",
        /// Provider quota type.
        quota_type: String => "Type",
        /// Provider quota state.
        status: String => "Status",
        /// Package identifier.
        package_id: String => "PackageID",
        /// UTC time when the quota starts.
        start_time: ResponseField<QuotaTimestamp> => "StartTime",
        /// UTC time when the quota expires.
        end_time: ResponseField<QuotaTimestamp> => "EndTime",
        /// UTC time when the quota was created.
        created_at: ResponseField<QuotaTimestamp> => "CreatedAt",
        /// UTC time when the quota was updated, or the zero time.
        updated_at: ResponseField<QuotaTimestamp> => "UpdatedAt",
        /// Whether the quota automatically renews.
        auto_renew: ResponseField<bool> => "AutoRenew",
        /// Whether the quota belongs to a proof of concept.
        poc: ResponseField<bool> => "POC",
        /// Whether the quota is system-managed.
        system: ResponseField<bool> => "System",
        /// Configured auto-renew period.
        auto_renew_period: ResponseField<i64> => "AutoRenewPeriod",
    }
}

impl Client {
    /// Reads one status snapshot and retains available rows on partial failure.
    pub async fn get_quota_status(
        &self,
        ctx: &RequestContext,
        input: GetQuotaStatusInput,
    ) -> Result<GetQuotaStatusOutput, Failure<GetQuotaStatusOutput>> {
        let all = [Kind::Synthetic, Kind::Email, Kind::Sms];
        let kinds = match input.kind.to_ascii_lowercase().as_str() {
            "" => &all[..],
            "synthetic" => &all[..1],
            "email" => &all[1..2],
            "sms" => &all[2..],
            _ => {
                return Err(failure(
                    Error::new(ErrorClass::InvalidInput)
                        .with_outer_detail(ErrorDetail::InvalidQuotaKind),
                ));
            }
        };
        let now = self.session.clock().utc_now().to_offset(UtcOffset::UTC);
        let mut reader = Reader::new(self, ctx, OPERATION);
        let mut rows = Vec::with_capacity(kinds.len());
        let mut causes = Vec::new();
        let mut row_ids = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let retained_ids = reader.decision_ids.len();
            match status_row(*kind, now, &mut reader).await {
                Ok(row) => rows.push(row),
                Err(error) => {
                    reader.decision_ids.truncate(retained_ids);
                    rows.push(unavailable_row(*kind, &error));
                    causes.push(error);
                }
            }
            row_ids.push(reader.decision_ids[retained_ids..].to_vec());
        }
        for (index, ids) in row_ids.iter().enumerate() {
            if ids.iter().any(|id| reader.evidence.id_reflects(id)) {
                let error = reader.invalid();
                rows[index] = unavailable_row(kinds[index], &error);
                causes.push(error);
            }
        }
        let mut output = GetQuotaStatusOutput {
            as_of: ResponseField::Value(QuotaTimestamp(now)),
            items: rows,
            complete: ResponseField::Value(causes.is_empty()),
        };
        for row in &mut output.items {
            if let Some(ResponseField::Value(seconds)) = row.time_left_seconds {
                let minutes = (seconds / 60).unsigned_abs();
                let display = format!(
                    "{}d {}h {}m",
                    minutes / 1440,
                    minutes % 1440 / 60,
                    minutes % 60
                );
                if reader.evidence.text_reflects(&display) {
                    row.time_left_seconds = Some(ResponseField::Redacted);
                }
            }
        }
        let safe_output = reader.evidence.can_retain_partial() && output.screen(&reader.evidence);
        if !causes.is_empty() {
            let mut error =
                Error::new(ErrorClass::Api).with_outer_detail(ErrorDetail::QuotaStatusIncomplete);
            for cause in causes {
                error = error.with_cause(Error::new(cause.class()).with_cause(cause));
            }
            let error = error.screen_api(&|text| reader.evidence.text_reflects(text));
            return Err(Failure::new(
                error,
                safe_output.then_some(output),
                WriteDispatch::NotAWrite,
            ));
        }
        if !safe_output {
            return Err(failure(reader.invalid()));
        }
        Ok(output)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Synthetic,
    Email,
    Sms,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Synthetic => "Synthetic",
            Self::Email => "Email",
            Self::Sms => "SMS",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::Synthetic => "billing-api/v1/synthetic/quota",
            Self::Email => "billing-api/v1/email/quota",
            Self::Sms => "billing-api/v1/sms/quota",
        }
    }

    fn next(self) -> &'static str {
        match self {
            Self::Synthetic => "monitor get-synthetic-quota",
            Self::Email | Self::Sms => "monitor get-notification-quota",
        }
    }
}

fn unavailable_row(kind: Kind, error: &Error) -> QuotaStatusRow {
    let api = error.api_error();
    let mut row = empty_row(kind, "unavailable", "");
    row.error = Some(QuotaStatusError {
        code: api.map_or("RequestFailed", |api| api.code()).to_owned(),
        operation: OPERATION.to_owned(),
        status: ResponseField::Value(api.and_then(|api| api.status()).map_or(0, i64::from)),
    });
    row
}

fn empty_row(kind: Kind, status: &str, next: &str) -> QuotaStatusRow {
    QuotaStatusRow {
        kind: kind.name().into(),
        class_name: String::new(),
        package_name: String::new(),
        status: status.into(),
        quota: None,
        price: None,
        start_time: None,
        end_time: None,
        time_left_seconds: None,
        auto_renew: None,
        renew_window_open: None,
        next_safe_command: next.into(),
        error: None,
    }
}

async fn status_row(
    kind: Kind,
    now: OffsetDateTime,
    reader: &mut Reader<'_>,
) -> Result<QuotaStatusRow, Error> {
    let mut row = empty_row(kind, "none", kind.next());
    let raw = match reader.get(kind.source(), &[]).await {
        Ok(raw) => raw,
        Err(error)
            if error.has_class(ErrorClass::NotFound)
                && !error.is_body_too_large()
                && error
                    .detail::<super::super::wire::BodyReadFailure>()
                    .is_none() =>
        {
            return Ok(row);
        }
        Err(error) => return Err(error),
    };
    let mut quota = reader.decode(decode_quota(&raw))?;
    reader.decode(quota.normalize_utc())?;
    reader.decision_ids.extend([
        quota.id_fields.id.clone(),
        quota.id_fields.package_id.clone(),
    ]);
    match kind {
        Kind::Synthetic => synthetic_row(&mut row, &quota, now, reader).await?,
        Kind::Email | Kind::Sms => {
            let route = match kind {
                Kind::Email => "billing-api/v1/email/packages",
                _ => "billing-api/v1/sms/packages",
            };
            let raw = reader.get(route, &[]).await?;
            let packages = reader.decode(notification_packages(&raw))?;
            if !unique(packages.iter().map(|package| &package.id)) {
                return Err(reader.invalid());
            }
            if let Some(package) = packages
                .into_iter()
                .find(|package| package.id == quota.id_fields.package_id)
            {
                row.package_name = package.name;
                row.price = package.price.map(ResponseField::Value);
            }
        }
    }
    if [&quota.id_fields.id, &quota.id_fields.package_id]
        .into_iter()
        .any(|id| reader.evidence.id_reflects(id))
    {
        return Err(reader.invalid());
    }
    row.status.clone_from(&quota.id_fields.status);
    row.start_time = Some(ResponseField::Value(quota.start_time));
    row.end_time = Some(ResponseField::Value(quota.end_time));
    row.auto_renew = Some(ResponseField::Value(quota.auto_renew));
    row.time_left_seconds = Some(ResponseField::Value(
        (quota.end_time.0 - now).whole_seconds(),
    ));
    row.quota = Some(quota.into_model());
    Ok(row)
}

async fn synthetic_row(
    row: &mut QuotaStatusRow,
    quota: &DecodedQuota,
    now: OffsetDateTime,
    reader: &mut Reader<'_>,
) -> Result<(), Error> {
    let detail = read_synthetic_detail(quota, reader).await?;
    let fields = &quota.id_fields;
    reader.decision_ids.push(detail.class_id.clone());
    row.class_name = detail.class_name;
    row.price = detail.price.map(ResponseField::Value);
    if detail.price == Some(0.0) {
        let mut classes = read_classes(reader).await?;
        read_class_packages(reader, &mut classes).await?;
        let settings = read_settings(reader).await?;
        if status_eligibility(
            reader,
            &classes,
            &settings,
            &detail.class_id,
            &fields.package_id,
            detail.api,
            detail.location,
        )? {
            let remaining = quota.end_time.0 - now;
            let open = fields.status == ACTIVE
                && remaining > Duration::ZERO
                && remaining <= Duration::days(7);
            row.renew_window_open = Some(ResponseField::Value(open));
            if !quota.poc && !quota.auto_renew {
                match fields.status.as_str() {
                    ACTIVE if open => {
                        row.next_safe_command = "monitor renew-synthetic-quota".into()
                    }
                    "IN-TRASH" if remaining < Duration::ZERO => {
                        row.next_safe_command = "monitor recover-synthetic-quota".into()
                    }
                    _ => (),
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn read_synthetic_detail(
    quota: &DecodedQuota,
    reader: &mut Reader<'_>,
) -> Result<Detail, Error> {
    let fields = &quota.id_fields;
    if [&fields.id, &fields.package_id]
        .into_iter()
        .any(|id| reader.evidence.id_reflects(id))
    {
        return Err(reader.invalid());
    }
    let raw = reader
        .get("billing-api/v2/synthetic/{}/quota-detail", &[&fields.id])
        .await?;
    let detail = reader.decode(synthetic_detail(&raw, fields))?;
    if [&fields.id, &fields.package_id, &detail.class_id]
        .into_iter()
        .any(|id| reader.evidence.id_reflects(id))
    {
        return Err(reader.invalid());
    }
    Ok(detail)
}

pub(super) struct QuotaIds {
    pub(super) id: String,
    pub(super) package_id: String,
    pub(super) status: String,
}

pub(super) struct DecodedQuota {
    pub(super) id_fields: QuotaIds,
    name: String,
    quota_type: String,
    pub(super) start_time: QuotaTimestamp,
    pub(super) end_time: QuotaTimestamp,
    created_at: QuotaTimestamp,
    updated_at: QuotaTimestamp,
    pub(super) auto_renew: bool,
    pub(super) poc: bool,
    system: bool,
    auto_renew_period: i64,
}

impl DecodedQuota {
    fn normalize_utc(&mut self) -> Option<()> {
        for timestamp in [
            &mut self.start_time,
            &mut self.end_time,
            &mut self.created_at,
            &mut self.updated_at,
        ] {
            timestamp.0 = timestamp.0.checked_to_offset(UtcOffset::UTC)?;
        }
        Some(())
    }

    pub(super) fn into_model(self) -> Quota {
        Quota {
            id: self.id_fields.id,
            name: self.name,
            quota_type: self.quota_type,
            status: self.id_fields.status,
            package_id: self.id_fields.package_id,
            start_time: ResponseField::Value(self.start_time),
            end_time: ResponseField::Value(self.end_time),
            created_at: ResponseField::Value(self.created_at),
            updated_at: ResponseField::Value(self.updated_at),
            auto_renew: ResponseField::Value(self.auto_renew),
            poc: ResponseField::Value(self.poc),
            system: ResponseField::Value(self.system),
            auto_renew_period: ResponseField::Value(self.auto_renew_period),
        }
    }
}

pub(super) fn decode_quota(raw: &serde_json::value::RawValue) -> Option<DecodedQuota> {
    let o = Object::of(raw)?;
    let id = o.text("id")?;
    let package_id = o.text("packageId")?;
    if !PathIds::Strict.allows(&id) || !PathIds::WithUnderscore.allows(&package_id) {
        return None;
    }
    let flag = |name| match o.integer(name)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    Some(DecodedQuota {
        id_fields: QuotaIds {
            id,
            package_id,
            status: o.text("status")?,
        },
        name: o.text("name")?,
        quota_type: o.text("type")?,
        start_time: token(o.required("startTime")?)?,
        end_time: token(o.required("endTime")?)?,
        created_at: token(o.required("createdAt")?)?,
        updated_at: token::<Option<_>>(o.required("updatedAt")?)?.unwrap_or_default(),
        auto_renew: flag("autoRenew")?,
        poc: flag("poc")?,
        system: token(o.required("system")?)?,
        auto_renew_period: o.integer("autoRenewPeriod")?,
    })
}

pub(super) struct Detail {
    pub(super) class_id: String,
    class_name: String,
    pub(super) api: i64,
    pub(super) location: i64,
    pub(super) price: Option<f64>,
}

fn synthetic_detail(raw: &serde_json::value::RawValue, quota: &QuotaIds) -> Option<Detail> {
    let o = Object::of(raw)?;
    if o.text("resourceId")? != quota.id || o.text("packageId")? != quota.package_id {
        return None;
    }
    let class_id = o.text("classId")?;
    if !PathIds::Strict.allows(&class_id) {
        return None;
    }
    token::<Option<String>>(o.required("tierName")?)?;
    for channel in ["email", "sms", "slack", "webhook", "telegram", "teams"] {
        let value = o.required(channel)?;
        if value.get() != "null" && integer(value)? < 0 {
            return None;
        }
    }
    Some(Detail {
        class_id,
        class_name: o.text("className")?,
        api: o.integer("api")?,
        location: o.integer("location")?,
        price: o.price()?,
    })
}

pub(super) struct NotificationPackage {
    pub(super) id: String,
    name: String,
    pub(super) price: Option<f64>,
}

pub(super) fn notification_packages(
    raw: &serde_json::value::RawValue,
) -> Option<Vec<NotificationPackage>> {
    let mut packages = Vec::new();
    for item in array(raw)? {
        let o = Object::of(item)?;
        let id = o.text("id")?;
        if !PathIds::WithUnderscore.allows(&id) {
            return None;
        }
        o.integer("amount")?;
        packages.push(NotificationPackage {
            id,
            name: o.text("name")?,
            price: o.price()?,
        });
    }
    Some(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_serializes_with_retained_offset() {
        let now = OffsetDateTime::UNIX_EPOCH.to_offset(UtcOffset::from_hms(7, 0, 0).unwrap());
        assert_eq!(
            serde_json::to_string(&QuotaTimestamp(now)).unwrap(),
            "\"1970-01-01T07:00:00+07:00\""
        );
    }

    #[test]
    fn utc_view_serializes_as_utc() {
        let now = OffsetDateTime::UNIX_EPOCH.to_offset(UtcOffset::from_hms(7, 0, 0).unwrap());
        let utc = QuotaTimestamp(now).to_utc().unwrap();
        assert_eq!(
            serde_json::to_string(&utc).unwrap(),
            "\"1970-01-01T00:00:00Z\""
        );
    }
}
