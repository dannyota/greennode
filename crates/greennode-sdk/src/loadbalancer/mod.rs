//! Read regional load balancers, their catalogs and children, and price quotes.

mod certificate_import;
mod child_deletes;
mod deletes;
mod models;
mod paid;
mod quotes;
mod types;
mod wire;
mod write;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::config::Session;
use crate::{
    Config, DEFAULT_PAGE, DEFAULT_PAGE_SIZE, Error, Failure, List, RequestContext, WriteDispatch,
};

pub use certificate_import::*;
pub use child_deletes::*;
pub use deletes::*;
pub use models::*;
pub use paid::*;
pub use quotes::{CreateLoadBalancerInput, ResizeLoadBalancerInput};
pub use types::*;
pub use wire::DecodeError;
use wire::Read;

/// Shares the configuration's session and connections for regional load balancers.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    pricing: crate::pricing::Client,
    writes: Arc<deletes::WriteLocks>,
}

#[expect(
    clippy::boxed_local,
    reason = "Private request errors are boxed until building a public Failure"
)]
fn failure<T>(error: Box<Error>) -> Failure<T> {
    Failure::new(*error, None, WriteDispatch::NotAWrite)
}

fn page_query(name: &str, page: i64, size: i64) -> [(&str, String); 3] {
    let page = if page > 0 { page } else { DEFAULT_PAGE };
    let size = if size > 0 { size } else { DEFAULT_PAGE_SIZE };
    [
        ("name", name.into()),
        ("page", page.to_string()),
        ("size", size.to_string()),
    ]
}

macro_rules! child_read {
    ($(#[$meta:meta])* $method:ident, $go:literal, $route:literal,
        $input:ty, $output:ident, $shape:ident $(($field:ident))?, [$($name:literal => $id:ident),+]) => {
        $(#[$meta])*
        pub async fn $method(
            &self,
            ctx: &RequestContext,
            input: $input,
        ) -> Result<$output, Failure<$output>> {
            let read = Read { id: concat!("loadbalancer.", $go), route: $route };
            let document = self.read(ctx, &read, &[$(($name, &input.$id)),+], &[])
                .await
                .map_err(failure)?;
            let envelope = document.object(&read).map_err(failure)?;
            child_read!(@output envelope, read, document, $shape $(($output, $field))?)
        }
    };
    (@output $envelope:ident, $read:ident, $document:ident, children) => {
        $envelope.children(&$read, &$document.evidence).map(|items| List { items }).map_err(failure)
    };
    (@output $envelope:ident, $read:ident, $document:ident, field($output:ident, $field:ident)) => {
        $envelope.field(&$read, &$document.evidence, "data").map(|$field| $output { $field }).map_err(failure)
    };
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates the client without performing any I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            pricing: crate::pricing::Client::new(config),
            writes: Arc::default(),
        }
    }

    /// Reads one page of load balancers in the active project.
    pub async fn list_load_balancers(
        &self,
        ctx: &RequestContext,
        input: ListLoadBalancersInput,
    ) -> Result<ListLoadBalancersOutput, Failure<ListLoadBalancersOutput>> {
        let read = Read {
            id: "loadbalancer.ListLoadBalancers",
            route: "v2/{}/loadBalancers",
        };
        let query = page_query(&input.name, input.page, input.size);
        let query = query.each_ref().map(|(key, value)| (*key, value.as_str()));
        let document = self.read(ctx, &read, &[], &query).await.map_err(failure)?;
        let page = document
            .object(&read)
            .and_then(|data| data.page::<ZonedLoadBalancer>(&read, &document.evidence))
            .map_err(failure)?;
        Ok(crate::PagedList {
            items: page.items.into_iter().map(|row| row.0).collect(),
            page: page.page,
            page_size: page.page_size,
            total_page: page.total_page,
            total_item: page.total_item,
        })
    }

    /// Reads a regional load balancer by ID.
    pub async fn get_load_balancer(
        &self,
        ctx: &RequestContext,
        input: GetLoadBalancerInput,
    ) -> Result<GetLoadBalancerOutput, Failure<GetLoadBalancerOutput>> {
        let read = Read {
            id: "loadbalancer.GetLoadBalancer",
            route: "v2/{}/loadBalancers/{}",
        };
        let document = self
            .read(
                ctx,
                &read,
                &[("LoadBalancerID", &input.load_balancer_id)],
                &[],
            )
            .await
            .map_err(failure)?;
        let ZonedLoadBalancer(load_balancer) = document
            .object(&read)
            .and_then(|data| data.field(&read, &document.evidence, "data"))
            .map_err(failure)?;
        Ok(GetLoadBalancerOutput { load_balancer })
    }

    /// Lists regional load balancer packages, optionally filtered by zone.
    pub async fn list_packages(
        &self,
        ctx: &RequestContext,
        input: ListPackagesInput,
    ) -> Result<ListPackagesOutput, Failure<ListPackagesOutput>> {
        let read = Read {
            id: "loadbalancer.ListPackages",
            route: "v2/{}/loadBalancers/packages",
        };
        let filter = [("zoneId", input.zone_id.as_str())];
        let query = if input.zone_id.is_empty() {
            &[][..]
        } else {
            &filter
        };
        let document = self.read(ctx, &read, &[], query).await.map_err(failure)?;
        let items = document
            .object(&read)
            .and_then(|data| data.field(&read, &document.evidence, "listData"))
            .map_err(failure)?;
        Ok(List { items })
    }

    /// Reads one page of certificate metadata.
    pub async fn list_certificates(
        &self,
        ctx: &RequestContext,
        input: ListCertificatesInput,
    ) -> Result<ListCertificatesOutput, Failure<ListCertificatesOutput>> {
        let read = Read {
            id: "loadbalancer.ListCertificates",
            route: "v2/{}/cas",
        };
        let query = page_query(&input.name, input.page, input.size);
        let query = query.each_ref().map(|(key, value)| (*key, value.as_str()));
        let document = self.read(ctx, &read, &[], &query).await.map_err(failure)?;
        document
            .object(&read)
            .and_then(|data| data.page(&read, &document.evidence))
            .map_err(failure)
    }

    /// Reads the metadata of one certificate.
    pub async fn get_certificate(
        &self,
        ctx: &RequestContext,
        input: GetCertificateInput,
    ) -> Result<GetCertificateOutput, Failure<GetCertificateOutput>> {
        let read = Read {
            id: "loadbalancer.GetCertificate",
            route: "v2/{}/cas/{}",
        };
        let document = self
            .read(ctx, &read, &[("CertificateID", &input.certificate_id)], &[])
            .await
            .map_err(failure)?;
        let certificate = document.direct(&read).map_err(failure)?;
        Ok(GetCertificateOutput { certificate })
    }

    child_read!(
        /// Lists listeners attached to a regional load balancer.
        list_listeners, "ListListeners", "v2/{}/loadBalancers/{}/listeners",
        ListListenersInput, ListListenersOutput, children, ["LoadBalancerID" => load_balancer_id]
    );
    child_read!(
        /// Reads one listener.
        get_listener, "GetListener", "v2/{}/loadBalancers/{}/listeners/{}",
        GetListenerInput, GetListenerOutput, field(listener),
        ["LoadBalancerID" => load_balancer_id, "ListenerID" => listener_id]
    );
    child_read!(
        /// Lists pools attached to a regional load balancer.
        list_pools, "ListPools", "v2/{}/loadBalancers/{}/pools",
        ListPoolsInput, ListPoolsOutput, children, ["LoadBalancerID" => load_balancer_id]
    );
    child_read!(
        /// Reads one pool.
        get_pool, "GetPool", "v2/{}/loadBalancers/{}/pools/{}",
        GetPoolInput, GetPoolOutput, field(pool),
        ["LoadBalancerID" => load_balancer_id, "PoolID" => pool_id]
    );
    child_read!(
        /// Reads a pool's health monitor.
        get_pool_health_monitor, "GetPoolHealthMonitor", "v2/{}/loadBalancers/{}/pools/{}/healthMonitor",
        GetPoolHealthMonitorInput, GetPoolHealthMonitorOutput, field(health_monitor),
        ["LoadBalancerID" => load_balancer_id, "PoolID" => pool_id]
    );
    child_read!(
        /// Lists members attached to a pool.
        list_pool_members, "ListPoolMembers", "v2/{}/loadBalancers/{}/pools/{}/members",
        ListPoolMembersInput, ListPoolMembersOutput, children,
        ["LoadBalancerID" => load_balancer_id, "PoolID" => pool_id]
    );
    child_read!(
        /// Lists layer 7 policies attached to a listener.
        list_policies, "ListPolicies", "v2/{}/loadBalancers/{}/listeners/{}/l7policies",
        ListPoliciesInput, ListPoliciesOutput, children,
        ["LoadBalancerID" => load_balancer_id, "ListenerID" => listener_id]
    );
    child_read!(
        /// Reads one layer 7 policy.
        get_policy, "GetPolicy", "v2/{}/loadBalancers/{}/listeners/{}/l7policies/{}",
        GetPolicyInput, GetPolicyOutput, field(policy),
        ["LoadBalancerID" => load_balancer_id, "ListenerID" => listener_id, "PolicyID" => policy_id]
    );

    /// Lists tags assigned to the specified load balancer.
    pub async fn list_tags(
        &self,
        ctx: &RequestContext,
        input: ListTagsInput,
    ) -> Result<ListTagsOutput, Failure<ListTagsOutput>> {
        let read = Read {
            id: "loadbalancer.ListTags",
            route: "v2/{}/tag/resource/{}",
        };
        let document = self
            .read(
                ctx,
                &read,
                &[("LoadBalancerID", &input.load_balancer_id)],
                &[],
            )
            .await
            .map_err(failure)?;
        let items = document.direct(&read).map_err(failure)?;
        Ok(List { items })
    }
}
