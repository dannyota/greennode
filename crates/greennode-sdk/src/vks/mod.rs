//! Read Kubernetes cluster inventory, supported versions, and account quota.

mod models;
#[cfg(test)]
mod tests;

pub use models::{Cluster, ClusterVersion, Quota};

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    operation::{self, Operation},
    redirect::{Classification, Retry},
    retry,
};
use crate::{
    ApiError, Config, Error, ErrorClass, Failure, List, LoginError, PagedList, RequestContext,
    ServiceAccountLoginError, WriteDispatch,
};
use http::{Method, StatusCode};
use models::{ClusterPage, QuotaResponse, Versions};
use serde::Serialize;
use std::sync::Arc;

const CLUSTERS: &str = "vks.ListClusters";
const VERSIONS: &str = "vks.ListClusterVersions";
const QUOTA: &str = "vks.GetQuota";

/// Client for account-wide Kubernetes reads in a supported region.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    supported_region: bool,
}

/// Selects a single zero-based page of cluster inventory.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListClustersInput {
    /// Zero-based page number, at most 2,147,483,647.
    pub page: i64,
    /// Page size, at most 2,147,483,647. Zero selects 10.
    pub size: i64,
}

/// A cluster page. The page number is zero-based.
pub type ListClustersOutput = PagedList<Cluster>;

/// Requests the version catalog with no filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListClusterVersionsInput;

/// The available cluster versions.
pub type ListClusterVersionsOutput = List<ClusterVersion>;

/// Requests the account quota with no filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetQuotaInput;

/// The configured region's account quota.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GetQuotaOutput {
    /// Account quota and current cluster usage.
    #[serde(rename = "Quota")]
    pub quota: Quota,
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
            supported_region: matches!(config.region(), "hcm-3" | "han-1"),
        }
    }

    /// Reads one page of clusters without selecting a project or fetching other pages.
    pub async fn list_clusters(
        &self,
        ctx: &RequestContext,
        input: ListClustersInput,
    ) -> Result<ListClustersOutput, Failure<ListClustersOutput>> {
        self.base(CLUSTERS)?;
        let max = i64::from(i32::MAX);
        if !(0..=max).contains(&input.page) {
            return Err(local_error(
                CLUSTERS,
                ErrorClass::InvalidInput,
                "InvalidUsage",
                "invalid input: vks.ListClusters: Page must be between 0 and 2147483647",
            ));
        }
        if !(0..=max).contains(&input.size) {
            return Err(local_error(
                CLUSTERS,
                ErrorClass::InvalidInput,
                "InvalidUsage",
                "invalid input: vks.ListClusters: Size must be between 1 and 2147483647, or 0 for the default",
            ));
        }
        let page = input.page.to_string();
        let size = if input.size == 0 { 10 } else { input.size }.to_string();
        let response: ClusterPage = self
            .read(
                ctx,
                &read_operation(CLUSTERS, "v1/clusters"),
                &[("page", &page), ("pageSize", &size)],
            )
            .await?;
        let page_size = i64::from(response.page_size);
        if response.page < 0 || page_size <= 0 || response.total < 0 {
            return Err(read_failure(malformed(CLUSTERS)));
        }
        Ok(PagedList {
            items: response.items,
            page: i64::from(response.page),
            page_size,
            total_page: response.total.div_euclid(page_size)
                + i64::from(response.total.rem_euclid(page_size) != 0),
            total_item: response.total,
        })
    }

    /// Reads the configured region's Kubernetes version catalog.
    pub async fn list_cluster_versions(
        &self,
        ctx: &RequestContext,
        _input: ListClusterVersionsInput,
    ) -> Result<ListClusterVersionsOutput, Failure<ListClusterVersionsOutput>> {
        let Versions(items) = self
            .read(ctx, &read_operation(VERSIONS, "v1/cluster-versions"), &[])
            .await?;
        Ok(List { items })
    }

    /// Reads the configured region's account quota.
    pub async fn get_quota(
        &self,
        ctx: &RequestContext,
        _input: GetQuotaInput,
    ) -> Result<GetQuotaOutput, Failure<GetQuotaOutput>> {
        let quota: QuotaResponse = self
            .read(ctx, &read_operation(QUOTA, "v1/quota"), &[])
            .await?;
        Ok(GetQuotaOutput {
            quota: quota.into(),
        })
    }

    fn base<O>(&self, operation: &'static str) -> Result<&crate::endpoint::Base, Failure<O>> {
        self.session
            .endpoints()
            .vks
            .as_ref()
            .filter(|_| self.supported_region)
            .ok_or_else(|| {
                local_error(
                    operation,
                    ErrorClass::InvalidConfig,
                    "InvalidConfig",
                    "invalid config: VKS requires hcm-3 or han-1",
                )
            })
    }

    async fn read<T: operation::Output, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let base = self.base(operation.id)?;
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| read_failure(*error))?;
        operation::execute(
            runtime,
            base,
            operation,
            &[],
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, output)| output)
        .map_err(|failure| read_failure(safe_error(operation.id, failure.error())))
    }
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
        allow_empty: false,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: true,
        reject_reflected_credentials: true,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}

fn read_failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

fn local_error<T>(
    operation: &'static str,
    class: ErrorClass,
    code: &'static str,
    message: &'static str,
) -> Failure<T> {
    read_failure(Error::new(class).with_cause(Error::from_api(ApiError::new(
        operation,
        None,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))))
}

fn api_error(
    operation: &'static str,
    status: Option<u16>,
    message: &'static str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::new(
        operation,
        status,
        SafeText::new("", &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

fn malformed(operation: &'static str) -> Error {
    api_error(
        operation,
        Some(200),
        "malformed response; body withheld",
        false,
    )
}

fn safe_error(operation: &'static str, error: &Error) -> Error {
    let class = error.class();
    const AUTH_FAILURE: &str = "VKS authentication failed; details withheld";
    if let Some(login) = error.login_error() {
        return Error::new(class).with_login(LoginError::new(
            AUTH_FAILURE,
            login.status(),
            login.captcha_suspected(),
        ));
    }
    if let Some(login) = error.service_account_login_error() {
        return Error::new(class)
            .with_service_login(ServiceAccountLoginError::new(AUTH_FAILURE, login.status()));
    }

    let api = error.api_error();
    let status = error
        .detail::<StatusCode>()
        .map(StatusCode::as_u16)
        .or_else(|| api.and_then(ApiError::status));
    let terminated = matches!(class, ErrorClass::Cancelled | ErrorClass::DeadlineExceeded);
    if status == Some(200) && !terminated {
        return malformed(operation);
    }
    let safe = api_error(
        operation,
        status,
        "request failed; response withheld",
        api.is_some_and(ApiError::retryable),
    );
    if class == ErrorClass::Api {
        safe
    } else {
        Error::new(class).with_cause(safe)
    }
}
