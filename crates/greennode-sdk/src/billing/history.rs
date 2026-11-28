mod models;
mod resources;
mod security;

pub use models::{
    DIRECTION_CREDIT, DIRECTION_DEBIT, GATEWAY_BALANCE, Payment, TRANSACTION_CHARGE,
    TRANSACTION_PURCHASE, TRANSACTION_REFUND, Transaction,
};
pub use resources::{
    ListResourcesInput, ListResourcesOutput, RENEW_TYPE_AUTO_RENEW, RENEW_TYPE_MANUAL,
    RENEW_TYPE_NON_RENEWABLE, Resource, ResourceBillingElement, ResourceThresholds,
};

use super::{
    Client,
    model::{field, reflects_integer},
    read_operation,
};
use crate::wire::{
    product::{
        lossy,
        operation::{self, Operation, Output},
        sender::{Response, SentCredentials},
    },
    strict_json::{self, Shape},
};
use crate::{Clock, Error, ErrorClass, Failure, PagedList, RequestContext, WriteDispatch};
use http::StatusCode;
use models::HistoryRow;
use security::{Redactor, invalid_response, response_error, safe_request_error};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{collections::HashSet, time::Duration};
use time::{Date, UtcOffset};

const PAGE_SIZE: i64 = 50;
const MAX_PAGES: i64 = 100;
const MAX_BYTES: usize = 32 << 20;
const TIMEOUT: Duration = Duration::from_secs(120);

/// Selects wallet transactions in an inclusive range.
#[derive(Clone)]
pub struct ListTransactionsInput {
    /// Required start date in YYYY-MM-DD form, in fixed UTC+07:00.
    pub start_date: String,
    /// Inclusive end date. An empty value means today in fixed UTC+07:00.
    pub end_date: String,
    /// One-based page number. Nonpositive values select page 1.
    pub page: i64,
    /// Page size. Nonpositive values and values above 50 select 50.
    pub size: i64,
    /// Reads the full range from page 1. A page above 1 is invalid with this option.
    pub all: bool,
}

crate::error::debug_record!(ListTransactionsInput {
    start_date,
    end_date,
    page,
    size,
    all
});

impl ListTransactionsInput {
    /// Sets the required start date and selects the first page through today.
    pub fn new(start_date: impl Into<String>) -> Self {
        Self {
            start_date: start_date.into(),
            end_date: String::new(),
            page: 0,
            size: 0,
            all: false,
        }
    }
}

/// Wallet transactions in API order and one-based page metadata.
pub type ListTransactionsOutput = PagedList<Transaction>;

/// Selects payments in an inclusive range.
#[derive(Clone)]
pub struct ListPaymentsInput {
    /// Required start date in YYYY-MM-DD form, in fixed UTC+07:00.
    pub start_date: String,
    /// Inclusive end date. An empty value means today in fixed UTC+07:00.
    pub end_date: String,
    /// One-based page number. Nonpositive values select page 1.
    pub page: i64,
    /// Page size. Nonpositive values and values above 50 select 50.
    pub size: i64,
    /// Reads the full range from page 1. A page above 1 is invalid with this option.
    pub all: bool,
}

crate::error::debug_record!(ListPaymentsInput {
    start_date,
    end_date,
    page,
    size,
    all
});

impl ListPaymentsInput {
    /// Sets the required start date and selects the first page through today.
    pub fn new(start_date: impl Into<String>) -> Self {
        Self {
            start_date: start_date.into(),
            end_date: String::new(),
            page: 0,
            size: 0,
            all: false,
        }
    }
}

/// Payments in API order and one-based page metadata.
pub type ListPaymentsOutput = PagedList<Payment>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads wallet transactions. An unconfirmed walk withholds all results.
    pub async fn list_transactions(
        &self,
        ctx: &RequestContext,
        input: ListTransactionsInput,
    ) -> Result<ListTransactionsOutput, Failure<ListTransactionsOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_history::<Transaction>(
                ctx,
                &history_operation("billing.ListTransactions", "gateway/api/v1/transactions"),
                input,
            )
            .await
            .map_err(|error| failure(*error))
        })
    }

    /// Reads payment history independently of wallet transactions.
    pub async fn list_payments(
        &self,
        ctx: &RequestContext,
        input: ListPaymentsInput,
    ) -> Result<ListPaymentsOutput, Failure<ListPaymentsOutput>> {
        crate::wire::product::exit::finish!(async {
            self.list_history::<Payment>(
                ctx,
                &history_operation("billing.ListPayments", "gateway/api/v1/payments"),
                ListTransactionsInput {
                    start_date: input.start_date,
                    end_date: input.end_date,
                    page: input.page,
                    size: input.size,
                    all: input.all,
                },
            )
            .await
            .map_err(|error| failure(*error))
        })
    }

    async fn list_history<W: HistoryRow>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        input: ListTransactionsInput,
    ) -> Result<PagedList<W>, Box<Error>> {
        let clock = self.session.clock();
        let ctx = ctx.child_with_deadline(clock.monotonic_now() + TIMEOUT);
        let invalid_input = || Box::new(Error::new(ErrorClass::InvalidInput));
        let (start, end) =
            history_dates(&input.start_date, &input.end_date, clock).ok_or_else(invalid_input)?;
        if input.all && input.page > 1 {
            return Err(invalid_input());
        }
        let mut page = input.page.max(1);
        let size = if (1..=PAGE_SIZE).contains(&input.size) {
            input.size
        } else {
            PAGE_SIZE
        };
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut credentials = SentCredentials::default();
        let mut frozen = None;
        let mut bytes = 0;
        let byte_limit = self.session.retained_byte_limit(MAX_BYTES);
        let incomplete = || {
            Box::new(
                Error::new(ErrorClass::HistoryIncomplete).with_cause(response_error(
                    operation,
                    None,
                    "HistoryIncomplete",
                    "billing history is incomplete; result withheld",
                    false,
                )),
            )
        };
        let invalid = || Box::new(invalid_response(operation, Some(StatusCode::OK)));
        let check_context = || {
            ctx.check(clock)
                .map_or(Ok(()), |end| Err(Box::new(end.into())))
        };
        let metadata = loop {
            let reply: HistoryEnvelope = self
                .history_request(
                    &ctx,
                    operation,
                    &[
                        ("dateStart", &start),
                        ("dateEnd", &end),
                        ("pageNumber", &page.to_string()),
                        ("pageSize", &size.to_string()),
                    ],
                    security::HISTORY_WITHHELD,
                )
                .await?;
            bytes += reply.bytes;
            if input.all && bytes > byte_limit {
                return Err(incomplete());
            }
            check_context()?;
            let data = reply.data.as_deref().ok_or_else(invalid)?;
            let text = crate::model::wire::go_json(data.get().as_bytes());
            let decoded = decode_history_page(&text).map_err(|_| invalid())?;
            let meta = decoded.pageable;
            let count = decoded.content.len() as i64;
            let expected_pages = if meta.total > 0 {
                (meta.total - 1) / meta.size.max(1) + 1
            } else {
                0
            };
            if meta.number <= 0
                || !(1..=size).contains(&meta.size)
                || meta.total < 0
                || meta.pages != expected_pages
                || count > meta.size
            {
                return Err(invalid());
            }
            let expected_rows = match page.cmp(&meta.pages) {
                std::cmp::Ordering::Less => meta.size,
                std::cmp::Ordering::Equal => (meta.total - 1) % meta.size + 1,
                std::cmp::Ordering::Greater => 0,
            };
            if meta.number != page || count != expected_rows {
                return Err(if input.all { incomplete() } else { invalid() });
            }
            if input.all {
                let coverage = (meta.size, meta.pages, meta.total);
                if meta.pages > MAX_PAGES || *frozen.get_or_insert(coverage) != coverage {
                    return Err(incomplete());
                }
            }
            let mut local = HashSet::new();
            for row in decoded.content {
                check_context()?;
                let row: W = lossy::from_slice(row.get().as_bytes()).map_err(|_| invalid())?;
                if !local.insert(row.id()) {
                    return Err(invalid());
                }
                items.push(row);
            }
            if input.all && local.into_iter().any(|id| !seen.insert(id)) {
                return Err(incomplete());
            }
            credentials.extend(&reply.credentials);
            if !input.all || page >= meta.pages {
                break meta;
            }
            page += 1;
        };
        let count = items.len() as i64;
        if input.all && count != metadata.total {
            return Err(incomplete());
        }
        check_context()?;
        let mut output = if input.all {
            PagedList {
                items,
                page: 1,
                page_size: count,
                total_page: 1,
                total_item: count,
            }
        } else {
            PagedList {
                items,
                page,
                page_size: metadata.size,
                total_page: metadata.pages,
                total_item: metadata.total,
            }
        };
        if [
            output.page,
            output.page_size,
            output.total_page,
            output.total_item,
        ]
        .into_iter()
        .any(|value| reflects_integer(value, &credentials))
        {
            return Err(invalid());
        }
        let redactor = Redactor::new(&credentials);
        for item in &mut output.items {
            check_context()?;
            if item.reflects_numbers(&credentials) {
                return Err(invalid());
            }
            item.redact(&redactor);
        }
        check_context()?;
        Ok(output)
    }

    async fn history_request<T: Output>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        query: &[(&str, &str)],
        message: &'static str,
    ) -> Result<T, Box<Error>> {
        let session = &self.session;
        let base = session
            .endpoints()
            .billing
            .as_ref()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session.runtime(ctx, &mut jitter)?;
        operation::execute::<T>(
            runtime,
            base,
            operation,
            &[],
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, page)| page)
        .map_err(|failure| {
            let (error, _, _) = failure.into_parts();
            Box::new(safe_request_error(operation, error, message))
        })
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

fn history_operation(id: &'static str, route: &'static str) -> Operation {
    Operation {
        sensitive: true,
        no_redirect: true,
        report_redirect: true,
        reflection: crate::wire::product::operation::Reflection::Redact,
        ..read_operation(id, route)
    }
}

pub(super) fn history_dates(start: &str, end: &str, clock: &dyn Clock) -> Option<(String, String)> {
    let start = parse_date(start)?;
    let end = if end.is_empty() {
        clock
            .utc_now()
            .checked_to_offset(UtcOffset::from_hms(7, 0, 0).ok()?)?
            .date()
    } else {
        parse_date(end)?
    };
    if start > end {
        return None;
    }
    let exclusive = end.next_day()?;
    if exclusive.year() > 9999 {
        return None;
    }
    let wire = |date: Date| {
        format!(
            "{:02}/{:02}/{:04}",
            date.day(),
            u8::from(date.month()),
            date.year()
        )
    };
    Some((wire(start), wire(exclusive)))
}

fn parse_date(text: &str) -> Option<Date> {
    let [y, y2, y3, y4, b'-', m, m2, b'-', d, d2] = text.as_bytes() else {
        return None;
    };
    if ![y, y2, y3, y4, m, m2, d, d2]
        .into_iter()
        .all(u8::is_ascii_digit)
    {
        return None;
    }
    Date::parse(
        text,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .ok()
}

struct Metadata {
    number: i64,
    size: i64,
    pages: i64,
    total: i64,
}

struct HistoryPage<'a> {
    content: Vec<&'a RawValue>,
    pageable: Metadata,
}

fn decode_history_page(data: &str) -> Result<HistoryPage<'_>, serde_json::Error> {
    let mut content = None;
    let mut metadata = None;
    visit_history_fields(
        &mut serde_json::Deserializer::from_str(data),
        &["content", "pageable"],
        |name, _, raw| {
            match name {
                "content" => content = serde_json::from_str(raw.get())?,
                "pageable" if raw.get() == "null" => metadata = None,
                "pageable" => {
                    const NAMES: &[&str] = &["number", "size", "totalPages", "totalElements"];
                    let fields = metadata.get_or_insert([None; 4]);
                    visit_history_fields(
                        &mut serde_json::Deserializer::from_str(raw.get()),
                        NAMES,
                        |name, _, raw| {
                            let index = NAMES.iter().position(|field| *field == name).unwrap();
                            fields[index] = serde_json::from_str(raw.get())?;
                            Ok(())
                        },
                    )?;
                }
                _ => unreachable!(),
            }
            Ok(())
        },
    )?;
    let missing = || serde::de::Error::custom("missing history field");
    let [number, size, pages, total] = metadata.ok_or_else(missing)?;
    Ok(HistoryPage {
        content: content.ok_or_else(missing)?,
        pageable: Metadata {
            number: number.ok_or_else(missing)?,
            size: size.ok_or_else(missing)?,
            pages: pages.ok_or_else(missing)?,
            total: total.ok_or_else(missing)?,
        },
    })
}

fn visit_history_fields<'de, D, F>(
    deserializer: D,
    names: &[&str],
    visit: F,
) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
    F: FnMut(&str, bool, &'de RawValue) -> Result<(), serde_json::Error>,
{
    struct Fields<'a, F> {
        names: &'a [&'a str],
        visit: F,
    }

    impl<'de, F> serde::de::Visitor<'de> for Fields<'_, F>
    where
        F: FnMut(&str, bool, &'de RawValue) -> Result<(), serde_json::Error>,
    {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a billing history object")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            mut self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if let Some(name) = self
                    .names
                    .iter()
                    .copied()
                    .find(|name| strict_json::field_matches(name, &key))
                {
                    (self.visit)(name, name == key, map.next_value::<&RawValue>()?)
                        .map_err(serde::de::Error::custom)?;
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }
    deserializer.deserialize_map(Fields { names, visit })
}

#[derive(Default, Deserialize)]
struct HistoryEnvelope {
    data: Option<Box<RawValue>>,
    #[serde(skip)]
    bytes: usize,
    #[serde(skip)]
    credentials: SentCredentials,
}

const ENVELOPE_SCHEMA: Shape<'static> = Shape::Object(&[field("code"), field("data")]);

fn envelope_code(raw: &RawValue) -> Option<i64> {
    match raw.get() {
        "null" => Some(200),
        text if text.starts_with('"') => serde_json::from_str::<String>(text).ok()?.parse().ok(),
        text => text.parse().ok(),
    }
}

impl Output for HistoryEnvelope {
    const SCHEMA: Shape<'static> = ENVELOPE_SCHEMA;

    fn credential_error(
        operation: &Operation,
        credentials: &SentCredentials,
    ) -> Option<Box<Error>> {
        credentials
            .has_numeric_product_credential()
            .then(|| Box::new(invalid_response(operation, None)))
    }

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let status = response.status;
        let invalid = || Box::new(invalid_response(operation, Some(status)));
        if !operation.success.contains(&status) {
            return Err(Box::new(security::status_error(
                operation,
                status,
                security::HISTORY_WITHHELD,
            )));
        }
        response.decode_projection(|body| {
            let raw = crate::go_json::raw_message(body)
                .ok()
                .flatten()
                .ok_or_else(invalid)?;
            let (code, data) = lossy::with_normalized(raw, |text| {
                let (mut code, mut data) = (None, None);
                visit_history_fields(
                    &mut serde_json::Deserializer::from_str(text),
                    &["code", "data"],
                    |name, _, raw| {
                        match name {
                            "code" => code = envelope_code(raw),
                            "data" if raw.get() == "null" => data = None,
                            "data" => data = Some(raw.to_owned()),
                            _ => unreachable!(),
                        }
                        Ok(())
                    },
                )?;
                Ok::<_, serde_json::Error>((code, data))
            })
            .map_err(|_| invalid())?;
            let code = code.ok_or_else(invalid)?;
            if !(200..=299).contains(&code) {
                if reflects_integer(code, &response.credentials) {
                    return Err(invalid());
                }
                return Err(Box::new(response_error(
                    operation,
                    Some(status),
                    &code.to_string(),
                    security::HISTORY_WITHHELD,
                    false,
                )));
            }
            Ok(Self {
                data,
                bytes: raw.len(),
                credentials: response.credentials.clone(),
            })
        })
    }
}
