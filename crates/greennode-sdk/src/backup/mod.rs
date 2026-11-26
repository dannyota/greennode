//! Read Backup Center storage backends and policies.

mod model;
#[cfg(test)]
mod tests;
mod wire;

pub use model::{Backend, DailyConfig, Policy, PolicyConfig};

use std::sync::Arc;

use http::{Method, StatusCode};
use serde::Serialize;

use crate::config::Session;
use crate::endpoint::Base;
use crate::wire::product::{
    operation::{self, Operation, Output},
    redirect::{Classification, Retry},
    retry,
};
use crate::{Config, Error, ErrorClass, Failure, PagedList, RequestContext, WriteDispatch};

use wire::{Collection, Detail, invalid_body, reflected_credential, withhold};

/// Client for token-scoped Backup Center reads in `hcm-3` and `han-1`.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

/// Input for the backend list, which takes no filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListBackendsInput;

/// Backend list with nullable page metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListBackendsOutput {
    /// Backends visible to the caller.
    pub items: Vec<Backend>,
    /// Page number when supplied.
    pub page: Option<i64>,
    /// Page size when supplied.
    pub page_size: Option<i64>,
    /// Total number of pages.
    pub total_page: i64,
    /// Total number of backends.
    pub total_item: i64,
}

/// Page selection for the policy list.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListPoliciesInput {
    /// One-based page number. Zero selects page 1.
    pub page: i64,
    /// Page size. Zero selects 200 items.
    pub size: i64,
}

/// One page of Backup Center policies.
pub type ListPoliciesOutput = PagedList<Policy>;

/// Identifies a Backup Center policy to read.
#[derive(Clone, Debug)]
pub struct GetPolicyInput {
    /// Policy ID using ASCII letters, digits, and hyphens.
    pub policy_id: String,
}

impl GetPolicyInput {
    /// Stores the policy ID for validation by [`Client::get_policy`].
    pub fn new(policy_id: impl Into<String>) -> Self {
        Self {
            policy_id: policy_id.into(),
        }
    }
}

/// The requested Backup Center policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GetPolicyOutput {
    /// Policy and its verified schedule settings.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Shares authentication and connections with the configuration without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }

    /// Reads the unpaged backend list using token scope.
    pub async fn list_backends(
        &self,
        ctx: &RequestContext,
        _input: ListBackendsInput,
    ) -> Result<ListBackendsOutput, Failure<ListBackendsOutput>> {
        let base = self.base()?;
        let result: Collection<Backend, Option<i64>> = self
            .read(
                ctx,
                base,
                &read_operation("backup.ListBackends", "v1/backends"),
                &[],
                &[],
            )
            .await?;
        Ok(ListBackendsOutput {
            items: result.items,
            page: result.page,
            page_size: result.page_size,
            total_page: result.total_pages,
            total_item: result.total_items,
        })
    }

    /// Reads one page without project scope. Negative page and size values are refused.
    pub async fn list_policies(
        &self,
        ctx: &RequestContext,
        input: ListPoliciesInput,
    ) -> Result<ListPoliciesOutput, Failure<ListPoliciesOutput>> {
        let base = self.base()?;
        if input.page < 0 || input.size < 0 {
            return Err(failure(Error::new(ErrorClass::InvalidInput)));
        }
        let page = input.page.max(1).to_string();
        let size = if input.size == 0 { 200 } else { input.size }.to_string();
        let result: Collection<Policy, i64> = self
            .read(
                ctx,
                base,
                &read_operation("backup.ListPolicies", "v1/backup-policies"),
                &[],
                &[("page", &page), ("size", &size)],
            )
            .await?;
        Ok(PagedList {
            items: result.items,
            page: result.page,
            page_size: result.page_size,
            total_page: result.total_pages,
            total_item: result.total_items,
        })
    }

    /// Reads one policy without project scope and verifies the returned identifier.
    pub async fn get_policy(
        &self,
        ctx: &RequestContext,
        input: GetPolicyInput,
    ) -> Result<GetPolicyOutput, Failure<GetPolicyOutput>> {
        let base = self.base()?;
        let operation = read_operation("backup.GetPolicy", "v1/backup-policies/{}");
        let result: Detail = self
            .read(ctx, base, &operation, &[&input.policy_id], &[])
            .await?;
        if result.policy.id != input.policy_id {
            return Err(failure(invalid_body(&operation)));
        }
        if result.reflected {
            return Err(failure(reflected_credential(&operation)));
        }
        Ok(GetPolicyOutput {
            policy: result.policy,
        })
    }

    fn base<O>(&self) -> Result<&Base, Failure<O>> {
        self.session
            .endpoints()
            .backup_center
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))
    }

    async fn read<T: Output, O>(
        &self,
        ctx: &RequestContext,
        base: &Base,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute::<T>(
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
        .map_err(|error| {
            let (error, _, dispatch) = error.into_parts();
            Failure::new(withhold(operation, error), None, dispatch)
        })
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
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
        reject_reflected_credentials: false,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}
