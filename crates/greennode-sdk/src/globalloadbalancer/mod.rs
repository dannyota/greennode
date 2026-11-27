//! Manage global load balancers and read their catalogs, children, and usage.

mod child;
mod load_balancer_writes;
mod models;
mod wire;
mod write;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use http::{Method, StatusCode};
use serde::Serialize;

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::{
    ApiError, Config, DEFAULT_PAGE_SIZE, Error, ErrorClass, Failure, List, RequestContext,
    WriteDispatch,
};

pub use child::*;
pub use load_balancer_writes::*;
pub use models::*;
pub use wire::DecodeError;
use wire::{Field, WireField, response_error};

/// Shares the configuration's authentication and connections for global load balancers.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
}

/// Selects all global load balancer packages.
#[derive(Clone, Debug, Default)]
pub struct ListPackagesInput {}

/// Packages available to the account.
pub type ListPackagesOutput = List<Package>;

/// Selects all global load balancer regions.
#[derive(Clone, Debug, Default)]
pub struct ListRegionsInput {}

/// Regions available to the account.
pub type ListRegionsOutput = List<Region>;

/// Selects a page of global load balancers.
#[derive(Clone, Debug, Default)]
pub struct ListLoadBalancersInput {
    /// Name filter sent to the service, even when empty.
    pub name: String,
    /// Zero-based offset. Negative values select zero.
    pub offset: i64,
    /// Page size. Nonpositive values select the default size.
    pub limit: i64,
}

/// Selects a global load balancer by ID.
#[derive(Clone, Debug)]
pub struct GetLoadBalancerInput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
}

impl GetLoadBalancerInput {
    /// Selects the load balancer with the given identifier.
    pub fn new(load_balancer_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
        }
    }
}

/// The requested global load balancer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetLoadBalancerOutput {
    /// Global load balancer returned by the service.
    #[serde(rename = "LoadBalancer")]
    pub load_balancer: LoadBalancer,
}

/// Selects all pools in a global load balancer.
#[derive(Clone, Debug)]
pub struct ListPoolsInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
}

impl ListPoolsInput {
    /// Selects pools under the given load balancer.
    pub fn new(load_balancer_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
        }
    }
}

/// Pools in the specified load balancer.
pub type ListPoolsOutput = List<Pool>;

/// Selects all listeners in a global load balancer.
#[derive(Clone, Debug)]
pub struct ListListenersInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
}

impl ListListenersInput {
    /// Selects listeners under the given load balancer.
    pub fn new(load_balancer_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
        }
    }
}

/// Listeners in the specified load balancer.
pub type ListListenersOutput = List<Listener>;

/// Selects an individual listener in a global load balancer.
#[derive(Clone, Debug)]
pub struct GetListenerInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Target listener identifier.
    pub listener_id: String,
}

impl GetListenerInput {
    /// Selects the listener under the given load balancer.
    pub fn new(load_balancer_id: impl Into<String>, listener_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            listener_id: listener_id.into(),
        }
    }
}

/// The requested listener.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetListenerOutput {
    /// Listener returned by the service.
    #[serde(rename = "Listener")]
    pub listener: Listener,
}

/// Selects all member groups of a global pool.
#[derive(Clone, Debug)]
pub struct ListPoolMembersInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Target pool identifier.
    pub pool_id: String,
}

impl ListPoolMembersInput {
    /// Selects member groups under the given load balancer and pool.
    pub fn new(load_balancer_id: impl Into<String>, pool_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            pool_id: pool_id.into(),
        }
    }
}

/// Member groups in the specified pool.
pub type ListPoolMembersOutput = List<PoolMember>;

/// Selects an individual member group of a global pool.
#[derive(Clone, Debug)]
pub struct GetPoolMemberInput {
    /// Parent load balancer identifier.
    pub load_balancer_id: String,
    /// Parent pool identifier.
    pub pool_id: String,
    /// Target member group identifier.
    pub pool_member_id: String,
}

impl GetPoolMemberInput {
    /// Selects the member group under the given load balancer and pool.
    pub fn new(
        load_balancer_id: impl Into<String>,
        pool_id: impl Into<String>,
        pool_member_id: impl Into<String>,
    ) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            pool_id: pool_id.into(),
            pool_member_id: pool_member_id.into(),
        }
    }
}

/// The requested pool member group.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetPoolMemberOutput {
    /// Pool member group returned by the service.
    #[serde(rename = "PoolMember")]
    pub pool_member: PoolMember,
}

/// Selects a load balancer's usage history and optional filters.
#[derive(Clone, Debug)]
pub struct ListUsageHistoriesInput {
    /// Target load balancer identifier.
    pub load_balancer_id: String,
    /// Start time filter sent when nonempty.
    pub from: String,
    /// End time filter sent when nonempty.
    pub to: String,
    /// Usage type filter sent when nonempty.
    pub r#type: String,
}

impl ListUsageHistoriesInput {
    /// Selects usage history under the given load balancer without filters.
    pub fn new(load_balancer_id: impl Into<String>) -> Self {
        Self {
            load_balancer_id: load_balancer_id.into(),
            from: String::new(),
            to: String::new(),
            r#type: String::new(),
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a client using the configuration's existing session without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
        }
    }

    /// Reads the global load balancer package catalog.
    pub async fn list_packages(
        &self,
        ctx: &RequestContext,
        _: ListPackagesInput,
    ) -> Result<ListPackagesOutput, Failure<ListPackagesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation("globalloadbalancer.ListPackages", "v1/packages");
            let items = self.read(ctx, &operation, &[], &[]).await?;
            Ok(List { items })
        })
    }

    /// Reads the global load balancer region catalog.
    pub async fn list_regions(
        &self,
        ctx: &RequestContext,
        _: ListRegionsInput,
    ) -> Result<ListRegionsOutput, Failure<ListRegionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation("globalloadbalancer.ListRegions", "v1/regions");
            let items = self.read(ctx, &operation, &[], &[]).await?;
            Ok(List { items })
        })
    }

    /// Reads one page of global load balancers.
    pub async fn list_load_balancers(
        &self,
        ctx: &RequestContext,
        input: ListLoadBalancersInput,
    ) -> Result<ListLoadBalancersOutput, Failure<ListLoadBalancersOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.ListLoadBalancers",
                "v1/global-load-balancers",
            );
            let offset = input.offset.max(0).to_string();
            let limit = if input.limit > 0 {
                input.limit
            } else {
                DEFAULT_PAGE_SIZE
            }
            .to_string();
            self.read(
                ctx,
                &operation,
                &[],
                &[
                    ("name", &input.name),
                    ("offset", &offset),
                    ("limit", &limit),
                ],
            )
            .await
        })
    }

    /// Reads a global load balancer by ID.
    pub async fn get_load_balancer(
        &self,
        ctx: &RequestContext,
        input: GetLoadBalancerInput,
    ) -> Result<GetLoadBalancerOutput, Failure<GetLoadBalancerOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.GetLoadBalancer",
                "v1/global-load-balancers/{}",
            );
            let load_balancer = self
                .read(
                    ctx,
                    &operation,
                    &[("LoadBalancerID", &input.load_balancer_id)],
                    &[],
                )
                .await?;
            Ok(GetLoadBalancerOutput { load_balancer })
        })
    }

    /// Reads pools belonging to a global load balancer.
    pub async fn list_pools(
        &self,
        ctx: &RequestContext,
        input: ListPoolsInput,
    ) -> Result<ListPoolsOutput, Failure<ListPoolsOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.ListPools",
                "v1/global-load-balancers/{}/global-pools",
            );
            let items = self
                .read(
                    ctx,
                    &operation,
                    &[("LoadBalancerID", &input.load_balancer_id)],
                    &[],
                )
                .await?;
            Ok(List { items })
        })
    }

    /// Reads listeners belonging to a global load balancer.
    pub async fn list_listeners(
        &self,
        ctx: &RequestContext,
        input: ListListenersInput,
    ) -> Result<ListListenersOutput, Failure<ListListenersOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.ListListeners",
                "v1/global-load-balancers/{}/global-listeners",
            );
            let items = self
                .read(
                    ctx,
                    &operation,
                    &[("LoadBalancerID", &input.load_balancer_id)],
                    &[],
                )
                .await?;
            Ok(List { items })
        })
    }

    /// Reads an individual listener.
    pub async fn get_listener(
        &self,
        ctx: &RequestContext,
        input: GetListenerInput,
    ) -> Result<GetListenerOutput, Failure<GetListenerOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.GetListener",
                "v1/global-load-balancers/{}/global-listeners/{}",
            );
            let listener = self
                .read(
                    ctx,
                    &operation,
                    &[
                        ("LoadBalancerID", &input.load_balancer_id),
                        ("ListenerID", &input.listener_id),
                    ],
                    &[],
                )
                .await?;
            Ok(GetListenerOutput { listener })
        })
    }

    /// Reads member groups belonging to a global pool.
    pub async fn list_pool_members(
        &self,
        ctx: &RequestContext,
        input: ListPoolMembersInput,
    ) -> Result<ListPoolMembersOutput, Failure<ListPoolMembersOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.ListPoolMembers",
                "v1/global-load-balancers/{}/global-pools/{}/pool-members",
            );
            let items = self
                .read(
                    ctx,
                    &operation,
                    &[
                        ("LoadBalancerID", &input.load_balancer_id),
                        ("PoolID", &input.pool_id),
                    ],
                    &[],
                )
                .await?;
            Ok(List { items })
        })
    }

    /// Reads an individual global pool member group.
    pub async fn get_pool_member(
        &self,
        ctx: &RequestContext,
        input: GetPoolMemberInput,
    ) -> Result<GetPoolMemberOutput, Failure<GetPoolMemberOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.GetPoolMember",
                "v1/global-load-balancers/{}/global-pools/{}/pool-members/{}",
            );
            let pool_member = self
                .read(
                    ctx,
                    &operation,
                    &[
                        ("LoadBalancerID", &input.load_balancer_id),
                        ("PoolID", &input.pool_id),
                        ("PoolMemberID", &input.pool_member_id),
                    ],
                    &[],
                )
                .await?;
            Ok(GetPoolMemberOutput { pool_member })
        })
    }

    /// Reads usage history, passing any nonempty filters to the service.
    pub async fn list_usage_histories(
        &self,
        ctx: &RequestContext,
        input: ListUsageHistoriesInput,
    ) -> Result<ListUsageHistoriesOutput, Failure<ListUsageHistoriesOutput>> {
        crate::wire::product::exit::finish!(async {
            let operation = read_operation(
                "globalloadbalancer.ListUsageHistories",
                "v1/global-load-balancers/{}/usage-histories",
            );
            let query: Vec<_> = [
                ("from", input.from.as_str()),
                ("to", input.to.as_str()),
                ("type", input.r#type.as_str()),
            ]
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect();
            self.read(
                ctx,
                &operation,
                &[("LoadBalancerID", &input.load_balancer_id)],
                &query,
            )
            .await
        })
    }

    async fn read<T: WireField, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        ids: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let path = path_ids(operation, ids).map_err(|message| failure(invalid_input(message)))?;
        let session = &self.session;
        let base = session
            .endpoints()
            .global_load_balancer
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        let mut evidence = Evidence::new();
        let (_, Field(mut output)) = operation::execute_with_evidence::<Field<T>>(
            runtime,
            base,
            operation,
            &Options::DEFAULT,
            &path,
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
            &mut evidence,
        )
        .await
        .map_err(|result| failure(result.into_parts().0))?;
        if !output.redact(&evidence) {
            return Err(failure(response_error(
                operation,
                DecodeError::ReflectedCredential,
            )));
        }
        Ok(output)
    }
}

fn path_ids<'a>(operation: &Operation, ids: &[(&str, &'a str)]) -> Result<Vec<&'a str>, String> {
    if let Some((field, _)) = ids.iter().find(|(_, value)| value.is_empty()) {
        return Err(format!("{} requires {field}", operation.id));
    }
    if let Some((field, _)) = ids
        .iter()
        .find(|(_, value)| !Options::DEFAULT.path_ids.allows(value))
    {
        return Err(format!(
            "{} requires {field} to match ^[A-Za-z0-9-]+$",
            operation.id
        ));
    }
    Ok(ids.iter().map(|(_, value)| *value).collect())
}

fn invalid_input(message: String) -> Error {
    let message = format!("vngcloud: invalid input: {message}");
    Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(ApiError::without_fallback(
        "",
        None,
        SafeText::new("InvalidUsage", &[]),
        SafeText::new(&message, &[]),
        false,
    )))
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
        allow_empty: true,
        no_output: false,
        keep_status: false,
        overflow_overrides_status: false,
        sensitive: false,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}
