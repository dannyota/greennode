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

const GET_POLICY: &str = "backup.GetPolicy";

/// Client for token-scoped Backup Center reads in `hcm-3` and `han-1`.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

/// Input for the backend list, which takes no filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListBackendsInput;

/// Backend list with nullable page metadata.
#[derive(Clone, PartialEq, Eq, Serialize)]
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

crate::wire::product::output::public_fields!(ListBackendsOutput {
    items,
    page,
    page_size,
    total_page,
    total_item
});

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
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct GetPolicyOutput {
    /// Policy and its verified schedule settings.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

crate::wire::product::output::public_fields!(GetPolicyOutput { policy });

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
        crate::wire::product::exit::finish!(async {
            let base = self.base(
                "vngcloud: invalid config: backup.ListBackends: requires a supported Backup Center region",
            )?;
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
        })
    }

    /// Reads one page without project scope. Negative page and size values are refused.
    pub async fn list_policies(
        &self,
        ctx: &RequestContext,
        input: ListPoliciesInput,
    ) -> Result<ListPoliciesOutput, Failure<ListPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            let base = self.base(
                "vngcloud: invalid config: backup.ListPolicies: requires a supported Backup Center region",
            )?;
            if input.page < 0 || input.size < 0 {
                return Err(failure(Error::new(ErrorClass::InvalidInput).with_detail(
                    "vngcloud: invalid input: backup.ListPolicies: Page and Size must not be negative",
                )));
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
        })
    }

    /// Reads one policy without project scope and verifies the returned identifier.
    pub async fn get_policy(
        &self,
        ctx: &RequestContext,
        input: GetPolicyInput,
    ) -> Result<GetPolicyOutput, Failure<GetPolicyOutput>> {
        crate::wire::product::exit::finish!(async {
            let base = self.base(
                "vngcloud: invalid config: backup.GetPolicy: requires a supported Backup Center region",
            )?;
            if input.policy_id.is_empty()
                || !input
                    .policy_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(failure(Error::new(ErrorClass::InvalidInput).with_detail(
                    "vngcloud: invalid input: backup.GetPolicy requires PolicyID to match ^[A-Za-z0-9-]+$",
                )));
            }
            let operation = read_operation(GET_POLICY, "v1/backup-policies/{}");
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
        })
    }

    fn base<O>(&self, message: &'static str) -> Result<&Base, Failure<O>> {
        self.session
            .endpoints()
            .backup_center
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig).with_detail(message)))
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
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}
