mod accounting;
mod prepare;

use super::{Client, EstimateArithmeticError, EstimateInput, EstimateItem, EstimateOutput};
use crate::model::pricing::DiscoveryPrice;
use crate::quote::{
    catalog::{self, Catalog, Encryption, Image, StorageCatalog, StorageRegion, VolumeType, Zone},
    request::{self, Product, Request, decode},
};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const OP: &str = "pricing.Estimate";
type Shared<T> = Result<T, Arc<Error>>;
type Cache<T> = Option<Shared<T>>;

#[derive(Default)]
struct Memo {
    project: Cache<String>,
    zones: Cache<Vec<Zone>>,
    volumes: Cache<Vec<VolumeType>>,
    encryption: Cache<Vec<Encryption>>,
    images: HashMap<String, Cache<Vec<Image>>>,
    regions: Cache<Vec<StorageRegion>>,
    storage: HashMap<String, Cache<StorageCatalog>>,
}

struct Run<'a> {
    input: &'a EstimateInput,
    catalog: Catalog<'a>,
    region: &'a str,
    memo: Memo,
    output: EstimateOutput,
    errors: Vec<Arc<Error>>,
    stopped: bool,
}

struct QuoteRequest {
    storage: Option<String>,
    body: Value,
}

impl QuoteRequest {
    fn regional(body: Value) -> Self {
        Self {
            storage: None,
            body,
        }
    }

    fn key(&self) -> (Option<&str>, String) {
        (self.storage.as_deref(), self.body.to_string())
    }

    async fn send(&self, catalog: &mut Catalog<'_>) -> Result<decode::Quote, Box<Error>> {
        let raw = if let Some(id) = self.storage.as_deref() {
            catalog
                .exchange(Request {
                    operation: OP,
                    template: "billing-api/v2/price",
                    product: Product::Storage(Some(id)),
                    path: &[],
                    query: &[("region_id", id)],
                    body: Some(&self.body),
                })
                .await?
        } else {
            catalog
                .exchange(Request {
                    operation: OP,
                    template: "v1/price",
                    product: Product::Portal,
                    path: &[],
                    query: &[],
                    body: Some(&self.body),
                })
                .await?
        };
        let output = decode::estimate(OP, &catalog::value(OP, &raw)?, self.storage.is_some())?;
        catalog.rows(output.breakdown.len())?;
        Ok(output)
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Estimates the specified monthly costs without placing an order.
    /// Partial results are attached to the failure when requested and available.
    pub async fn estimate(
        &self,
        ctx: &RequestContext,
        input: EstimateInput,
    ) -> Result<EstimateOutput, Failure<EstimateOutput>> {
        let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
        super::validate::setup(&input, &self.region)
            .map_err(|field| failure(invalid_input(field)))?;
        let catalog = Catalog::new(&self.session, ctx, input.max_requests);
        let mut run = Run {
            input: &input,
            catalog,
            region: &self.region,
            memo: Memo::default(),
            output: accounting::initialize(&input, &self.region, self.session.clock().utc_now()),
            errors: Vec::new(),
            stopped: false,
        };
        let outcome = run.quote().await.and_then(|()| run.finish());
        let Run {
            output,
            errors,
            catalog,
            memo,
            ..
        } = run;
        drop(memo);
        let evidence = &catalog.evidence;
        if let Err(error) = outcome {
            drop(errors);
            return Err(failure(safe_error(recover(error), evidence)));
        }
        if let Some(first) = errors.first()
            && output
                .items
                .iter()
                .all(|item| item.price.status == "failed")
        {
            let error = Arc::clone(first);
            drop(errors);
            return Err(failure(safe_error(recover(error), evidence)));
        }
        if serde_json::to_value(&output).map_or(true, |value| evidence.output_reflects(&value))
            || accounting::table_amounts(&output).any(|amount| evidence.reflects_fixed_two(amount))
        {
            return Err(failure(safe_error(*request::invalid(OP, 200), evidence)));
        }
        if errors.is_empty() {
            return Ok(output);
        }
        let mut error = Error::new(ErrorClass::DiscoveryIncomplete);
        for cause in errors {
            if let Some(cause) = Arc::into_inner(cause) {
                error = error.with_cause(safe_error(cause, evidence));
            }
        }
        Err(Failure::new(error, Some(output), WriteDispatch::NotAWrite))
    }
}

impl Run<'_> {
    async fn quote(&mut self) -> Shared<()> {
        self.catalog
            .rows(self.output.items.len())
            .map_err(Arc::from)?;
        let requests = self.prepare().await?;
        if self.stopped {
            return Ok(());
        }
        let distinct = requests
            .iter()
            .flatten()
            .map(QuoteRequest::key)
            .collect::<HashSet<_>>()
            .len();
        if let Err(error) = self.catalog.preflight(distinct) {
            return self.handle("", Arc::from(error));
        }
        let mut memo = HashMap::new();
        for (index, request) in requests.iter().enumerate() {
            let Some(request) = request else {
                continue;
            };
            let key = request.key();
            if !memo.contains_key(&key) {
                memo.insert(
                    key.clone(),
                    request.send(&mut self.catalog).await.map_err(Arc::from),
                );
            }
            let Some(result) = memo.get(&key) else {
                continue;
            };
            let Some(item) = self.output.items.get_mut(index) else {
                continue;
            };
            let result = match result {
                Ok(quote) => accounting::account(
                    item,
                    quote,
                    &mut self.output.metadata.notes,
                    &self.catalog.evidence,
                ),
                Err(error) => Err(Arc::clone(error)),
            };
            if let Err(error) = result {
                let key = item.key.clone();
                self.handle(&key, error)?;
                if self.stopped {
                    break;
                }
            }
        }
        Ok(())
    }

    fn handle(&mut self, key: &str, error: Arc<Error>) -> Shared<()> {
        if error.has_class(ErrorClass::InvalidInput) || !self.input.allow_partial {
            return Err(error);
        }
        self.stopped |= [
            ErrorClass::Auth,
            ErrorClass::EstimateLimit,
            ErrorClass::RateLimited,
            ErrorClass::Cancelled,
            ErrorClass::DeadlineExceeded,
        ]
        .into_iter()
        .any(|class| error.has_class(class));
        self.record(key, error);
        Ok(())
    }
}

fn invalid_input(field: &'static str) -> Error {
    let message = format!("greennode: invalid input: pricing.Estimate: invalid {field}");
    Error::new(ErrorClass::InvalidInput).with_cause(request::api("", None, "", &message, false))
}

fn failed() -> DiscoveryPrice {
    DiscoveryPrice {
        status: "failed".into(),
        source: OP.into(),
        ..DiscoveryPrice::default()
    }
}

fn recover(error: Arc<Error>) -> Error {
    Arc::try_unwrap(error).unwrap_or_else(|error| Error::new(error.class()))
}

fn failure_code(error: &crate::ApiError) -> String {
    if error.code() == "InvalidResponse" {
        return error.code().into();
    }
    if error.status() == Some(200)
        && let Ok(code) = error.code().parse::<u16>()
        && (code == 112 || (400..=599).contains(&code))
    {
        return code.to_string();
    }
    match error.status() {
        Some(400) => "BadRequest",
        Some(401) => "Unauthorized",
        Some(403) => "Forbidden",
        Some(404) => "NotFound",
        Some(409) => "Conflict",
        Some(429) => "Throttled",
        Some(500..) => "ServerError",
        Some(400..) => "ClientError",
        _ => "",
    }
    .into()
}

fn safe_error(error: Error, evidence: &Evidence) -> Error {
    if let Some(arithmetic) = error.detail::<EstimateArithmeticError>()
        && evidence.text_reflects(&arithmetic.item_key)
    {
        let screened = EstimateArithmeticError {
            stage: arithmetic.stage,
            item_key: "<redacted>".into(),
        };
        return error.with_detail(screened);
    }
    if error.has_class(ErrorClass::InvalidInput)
        && error
            .api_error()
            .is_some_and(|api| api.operation().is_empty())
    {
        return error;
    }
    let Some(api) = error.api_error() else {
        return error;
    };
    if error.has_class(ErrorClass::EstimateLimit) || error.is_body_too_large() {
        return error;
    }
    let safe = request::api(
        api.operation(),
        api.status(),
        &failure_code(api),
        "estimate request failed; body withheld",
        api.retryable(),
    );
    [
        ErrorClass::InvalidInput,
        ErrorClass::Auth,
        ErrorClass::PermissionDenied,
        ErrorClass::NotFound,
        ErrorClass::RateLimited,
        ErrorClass::Cancelled,
        ErrorClass::DeadlineExceeded,
    ]
    .into_iter()
    .filter(|class| error.has_identity(*class))
    .fold(safe, |safe, class| Error::new(class).with_cause(safe))
}
