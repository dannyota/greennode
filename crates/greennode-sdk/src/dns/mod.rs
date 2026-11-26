//! Read and manage vDNS hosted zones and records.

mod models;
#[cfg(test)]
mod tests;
mod wire;
mod writes;

pub use models::{Field, HostedZone, Record, RecordValue, Timestamp, VpcMapRegion};
pub use wire::{DecodeError, Page};
pub use writes::{
    CreateHostedZoneInput, CreateHostedZoneOutput, CreateRecordInput, CreateRecordOutput,
    DeleteHostedZoneInput, DeleteHostedZoneOutput, DeleteRecordInput, DeleteRecordOutput,
    ErrorDetail, UpdateHostedZoneInput, UpdateHostedZoneOutput, UpdateRecordInput,
    UpdateRecordOutput,
};

use std::sync::Arc;

use http::{Method, StatusCode};
use serde::Serialize;

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::{
    ApiError, Config, DEFAULT_PAGE, DEFAULT_PAGE_SIZE, Error, ErrorClass, Failure, RequestContext,
    WriteDispatch,
};
use wire::{Detail, PageResponse};

const LIST_ZONES: &str = "dns.ListHostedZones";
const GET_ZONE: &str = "dns.GetHostedZone";
const LIST_RECORDS: &str = "dns.ListRecords";
const GET_RECORD: &str = "dns.GetRecord";

/// Client for account-wide vDNS hosted zones and records.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Selects one page of hosted zones in the account.
#[derive(Clone, Debug, Default)]
pub struct ListHostedZonesInput {
    /// Name filter. An empty value omits the filter.
    pub name: String,
    /// One-based page number. A nonpositive value selects 1.
    pub page: i64,
    /// Page size. A nonpositive value selects 10,000.
    pub size: i64,
}

/// A page of hosted zones.
pub type ListHostedZonesOutput = Page<HostedZone>;

/// Identifies one hosted zone to read.
#[derive(Clone, Debug)]
pub struct GetHostedZoneInput {
    /// Hosted zone identifier.
    pub hosted_zone_id: String,
}

impl GetHostedZoneInput {
    /// Selects the zone with the given identifier.
    pub fn new(hosted_zone_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
        }
    }
}

/// The requested hosted zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GetHostedZoneOutput {
    /// The returned zone.
    #[serde(rename = "HostedZone")]
    pub hosted_zone: HostedZone,
}

/// Selects the records in one hosted zone with an optional name filter.
#[derive(Clone, Debug)]
pub struct ListRecordsInput {
    /// Parent hosted zone identifier.
    pub hosted_zone_id: String,
    /// Name filter. An empty value omits the filter.
    pub name: String,
}

impl ListRecordsInput {
    /// Selects records in the given zone without a name filter.
    pub fn new(hosted_zone_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            name: String::new(),
        }
    }
}

/// Records returned for one hosted zone with the reported page metadata.
pub type ListRecordsOutput = Page<Record>;

/// Identifies one record to read in a hosted zone.
#[derive(Clone, Debug)]
pub struct GetRecordInput {
    /// Parent hosted zone identifier.
    pub hosted_zone_id: String,
    /// Record identifier.
    pub record_id: String,
}

impl GetRecordInput {
    /// Selects the given record within the given zone.
    pub fn new(hosted_zone_id: impl Into<String>, record_id: impl Into<String>) -> Self {
        Self {
            hosted_zone_id: hosted_zone_id.into(),
            record_id: record_id.into(),
        }
    }
}

/// The requested DNS record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GetRecordOutput {
    /// The returned record.
    #[serde(rename = "Record")]
    pub record: Record,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Shares the configuration's authentication and connections without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Reads one page of hosted zones.
    pub async fn list_hosted_zones(
        &self,
        ctx: &RequestContext,
        input: ListHostedZonesInput,
    ) -> Result<ListHostedZonesOutput, Failure<ListHostedZonesOutput>> {
        let page = page_value(input.page, DEFAULT_PAGE);
        let size = page_value(input.size, DEFAULT_PAGE_SIZE);
        let mut query = vec![("page", page.as_str()), ("size", size.as_str())];
        if !input.name.is_empty() {
            query.push(("name", &input.name));
        }
        let operation = read_operation(LIST_ZONES, "v1/dns/hosted-zone");
        let response: PageResponse<HostedZone> = self.read(ctx, &operation, &[], &query).await?;
        Ok(response.page)
    }

    /// Reads a hosted zone by identifier.
    pub async fn get_hosted_zone(
        &self,
        ctx: &RequestContext,
        input: GetHostedZoneInput,
    ) -> Result<GetHostedZoneOutput, Failure<GetHostedZoneOutput>> {
        check_ids(GET_ZONE, &[("HostedZoneID", &input.hosted_zone_id)])?;
        let operation = read_operation(GET_ZONE, "v1/dns/hosted-zone/{}");
        let response: Detail<HostedZone> = self
            .read(ctx, &operation, &[&input.hosted_zone_id], &[])
            .await?;
        Ok(GetHostedZoneOutput {
            hosted_zone: response.data,
        })
    }

    /// Reads records in one hosted zone. An incomplete list is returned as partial output.
    pub async fn list_records(
        &self,
        ctx: &RequestContext,
        input: ListRecordsInput,
    ) -> Result<ListRecordsOutput, Failure<ListRecordsOutput>> {
        check_ids(LIST_RECORDS, &[("HostedZoneID", &input.hosted_zone_id)])?;
        let filter = [("name", input.name.as_str())];
        let query = if input.name.is_empty() {
            &[][..]
        } else {
            &filter
        };
        let operation = read_operation(LIST_RECORDS, "v1/dns/hosted-zone/{}/record");
        let response: PageResponse<Record> = self
            .read(ctx, &operation, &[&input.hosted_zone_id], query)
            .await?;
        if response.incomplete {
            return Err(Failure::new(
                api_error(
                    "",
                    "",
                    "dns.ListRecords: incomplete record list; continuation parameters are unverified",
                ),
                Some(response.page),
                WriteDispatch::NotAWrite,
            ));
        }
        Ok(response.page)
    }

    /// Reads one record by identifier from the given hosted zone.
    pub async fn get_record(
        &self,
        ctx: &RequestContext,
        input: GetRecordInput,
    ) -> Result<GetRecordOutput, Failure<GetRecordOutput>> {
        check_ids(
            GET_RECORD,
            &[
                ("HostedZoneID", &input.hosted_zone_id),
                ("RecordID", &input.record_id),
            ],
        )?;
        let operation = read_operation(GET_RECORD, "v1/dns/hosted-zone/{}/record/{}");
        let response: Detail<Record> = self
            .read(
                ctx,
                &operation,
                &[&input.hosted_zone_id, &input.record_id],
                &[],
            )
            .await?;
        Ok(GetRecordOutput {
            record: response.data,
        })
    }

    async fn read<T: Output, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let session = &self.session;
        let base = session.endpoints().dns.as_ref().ok_or_else(|| {
            failure(Error::new(ErrorClass::InvalidConfig).with_cause(api_error(
                "",
                "InvalidConfig",
                "vngcloud: invalid config: DNS endpoint is unavailable",
            )))
        })?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute(
            runtime,
            base,
            operation,
            path,
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, output)| output)
        .map_err(|result| failure(result.into_parts().0))
    }
}

fn page_value(value: i64, default: i64) -> String {
    if value <= 0 { default } else { value }.to_string()
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn check_ids<T>(operation: &str, fields: &[(&str, &str)]) -> Result<(), Failure<T>> {
    validate_ids(operation, fields).map_err(|error| failure(*error))
}

fn validate_ids(operation: &str, fields: &[(&str, &str)]) -> Result<(), Box<Error>> {
    let message = if let Some((field, _)) = fields.iter().find(|(_, value)| value.is_empty()) {
        format!("vngcloud: invalid input: {operation} requires {field}")
    } else if let Some((field, _)) = fields
        .iter()
        .find(|(_, value)| !Options::DEFAULT.path_ids.allows(value))
    {
        format!("vngcloud: invalid input: {operation} requires {field} to match ^[A-Za-z0-9-]+$")
    } else {
        return Ok(());
    };
    Err(Box::new(
        Error::new(ErrorClass::InvalidInput).with_cause(api_error("", "InvalidUsage", &message)),
    ))
}

fn read_operation(id: &'static str, template: &'static str) -> Operation {
    Operation {
        id,
        template,
        method: Method::GET,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: &[StatusCode::OK],
        cap: 8 << 20,
        allow_recovery: true,
        allow_empty: true,
        no_output: false,
        keep_status: false,
        overflow_overrides_status: false,
        sensitive: false,
        reject_reflected_credentials: false,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

fn api_error(operation: &'static str, code: &str, message: &str) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation,
        None,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
}
