use super::models::*;
use crate::{List, PagedList};
use serde::Serialize;

/// Parameters for ListLoadBalancers.
#[derive(Clone, Debug, Default)]
pub struct ListLoadBalancersInput {
    /// Name filter sent to the API.
    pub name: String,
    /// One-based page number. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Nonpositive values select the default.
    pub size: i64,
}

/// Parameters for GetLoadBalancer.
#[derive(Clone, Debug, Default)]
pub struct GetLoadBalancerInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

/// Parameters for ListPackages.
#[derive(Clone, Debug, Default)]
pub struct ListPackagesInput {
    /// Optional zone filter.
    pub zone_id: String,
}

/// Parameters for ListCertificates.
#[derive(Clone, Debug, Default)]
pub struct ListCertificatesInput {
    /// Name filter sent to the API.
    pub name: String,
    /// One-based page number. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Nonpositive values select the default.
    pub size: i64,
}

/// Parameters for ListListeners.
#[derive(Clone, Debug, Default)]
pub struct ListListenersInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

/// Parameters for GetListener.
#[derive(Clone, Debug, Default)]
pub struct GetListenerInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
}

/// Parameters for ListPools.
#[derive(Clone, Debug, Default)]
pub struct ListPoolsInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

/// Parameters for GetPool.
#[derive(Clone, Debug, Default)]
pub struct GetPoolInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

/// Parameters for GetPoolHealthMonitor.
#[derive(Clone, Debug, Default)]
pub struct GetPoolHealthMonitorInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

/// Parameters for ListPoolMembers.
#[derive(Clone, Debug, Default)]
pub struct ListPoolMembersInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

/// Parameters for ListPolicies.
#[derive(Clone, Debug, Default)]
pub struct ListPoliciesInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
}

/// Parameters for GetPolicy.
#[derive(Clone, Debug, Default)]
pub struct GetPolicyInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
    /// Identifier of the policy.
    pub policy_id: String,
}

/// Parameters for ListTags.
#[derive(Clone, Debug, Default)]
pub struct ListTagsInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

/// Parameters for GetCertificate.
#[derive(Clone, Debug, Default)]
pub struct GetCertificateInput {
    /// Identifier of the certificate.
    pub certificate_id: String,
}

/// LoadBalancer items returned by the service.
pub type ListLoadBalancersOutput = PagedList<LoadBalancer>;

/// Package items returned by the service.
pub type ListPackagesOutput = List<Package>;

/// Certificate items returned by the service.
pub type ListCertificatesOutput = PagedList<Certificate>;

/// Listener items returned by the service.
pub type ListListenersOutput = List<Listener>;

/// Pool items returned by the service.
pub type ListPoolsOutput = List<Pool>;

/// PoolMember items returned by the service.
pub type ListPoolMembersOutput = List<PoolMember>;

/// Policy items returned by the service.
pub type ListPoliciesOutput = List<Policy>;

/// Tag items returned by the service.
pub type ListTagsOutput = List<Tag>;

/// The result of reading a load balancer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetLoadBalancerOutput {
    /// Load balancer returned by the service.
    #[serde(rename = "LoadBalancer")]
    pub load_balancer: LoadBalancer,
}

crate::wire::product::output::public_fields!(GetLoadBalancerOutput { load_balancer });

/// The result of reading a listener.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetListenerOutput {
    /// Listener returned by the service.
    #[serde(rename = "Listener")]
    pub listener: Listener,
}

crate::wire::product::output::public_fields!(GetListenerOutput { listener });

/// The result of reading a pool.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetPoolOutput {
    /// Pool returned by the service.
    #[serde(rename = "Pool")]
    pub pool: Pool,
}

crate::wire::product::output::public_fields!(GetPoolOutput { pool });

/// The result of reading a health monitor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetPoolHealthMonitorOutput {
    /// Health monitor returned by the service.
    #[serde(rename = "HealthMonitor")]
    pub health_monitor: HealthMonitor,
}

crate::wire::product::output::public_fields!(GetPoolHealthMonitorOutput { health_monitor });

/// The result of reading a policy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetPolicyOutput {
    /// Policy returned by the service.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

crate::wire::product::output::public_fields!(GetPolicyOutput { policy });

/// The result of reading a certificate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetCertificateOutput {
    /// Certificate returned by the service.
    #[serde(rename = "Certificate")]
    pub certificate: Certificate,
}

crate::wire::product::output::public_fields!(GetCertificateOutput { certificate });
