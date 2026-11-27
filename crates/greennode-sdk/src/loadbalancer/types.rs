use super::models::*;
use crate::{List, PagedList};
use serde::Serialize;

/// Parameters for ListLoadBalancers.
#[derive(Clone, Default)]
pub struct ListLoadBalancersInput {
    /// Name filter sent to the API.
    pub name: String,
    /// One-based page number. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Nonpositive values select the default.
    pub size: i64,
}

crate::error::debug_record!(ListLoadBalancersInput { name, page, size });

/// Parameters for GetLoadBalancer.
#[derive(Clone, Default)]
pub struct GetLoadBalancerInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

crate::error::debug_record!(GetLoadBalancerInput { load_balancer_id });

/// Parameters for ListPackages.
#[derive(Clone, Default)]
pub struct ListPackagesInput {
    /// Optional zone filter.
    pub zone_id: String,
}

crate::error::debug_record!(ListPackagesInput { zone_id });

/// Parameters for ListCertificates.
#[derive(Clone, Default)]
pub struct ListCertificatesInput {
    /// Name filter sent to the API.
    pub name: String,
    /// One-based page number. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Nonpositive values select the default.
    pub size: i64,
}

crate::error::debug_record!(ListCertificatesInput { name, page, size });

/// Parameters for ListListeners.
#[derive(Clone, Default)]
pub struct ListListenersInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

crate::error::debug_record!(ListListenersInput { load_balancer_id });

/// Parameters for GetListener.
#[derive(Clone, Default)]
pub struct GetListenerInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
}

crate::error::debug_record!(GetListenerInput {
    load_balancer_id,
    listener_id
});

/// Parameters for ListPools.
#[derive(Clone, Default)]
pub struct ListPoolsInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

crate::error::debug_record!(ListPoolsInput { load_balancer_id });

/// Parameters for GetPool.
#[derive(Clone, Default)]
pub struct GetPoolInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

crate::error::debug_record!(GetPoolInput {
    load_balancer_id,
    pool_id
});

/// Parameters for GetPoolHealthMonitor.
#[derive(Clone, Default)]
pub struct GetPoolHealthMonitorInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

crate::error::debug_record!(GetPoolHealthMonitorInput {
    load_balancer_id,
    pool_id
});

/// Parameters for ListPoolMembers.
#[derive(Clone, Default)]
pub struct ListPoolMembersInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the pool.
    pub pool_id: String,
}

crate::error::debug_record!(ListPoolMembersInput {
    load_balancer_id,
    pool_id
});

/// Parameters for ListPolicies.
#[derive(Clone, Default)]
pub struct ListPoliciesInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
}

crate::error::debug_record!(ListPoliciesInput {
    load_balancer_id,
    listener_id
});

/// Parameters for GetPolicy.
#[derive(Clone, Default)]
pub struct GetPolicyInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
    /// Identifier of the listener.
    pub listener_id: String,
    /// Identifier of the policy.
    pub policy_id: String,
}

crate::error::debug_record!(GetPolicyInput {
    load_balancer_id,
    listener_id,
    policy_id
});

/// Parameters for ListTags.
#[derive(Clone, Default)]
pub struct ListTagsInput {
    /// Identifier of the load balancer.
    pub load_balancer_id: String,
}

crate::error::debug_record!(ListTagsInput { load_balancer_id });

/// Parameters for GetCertificate.
#[derive(Clone, Default)]
pub struct GetCertificateInput {
    /// Identifier of the certificate.
    pub certificate_id: String,
}

crate::error::debug_record!(GetCertificateInput { certificate_id });

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
#[derive(Clone, PartialEq, Serialize)]
pub struct GetLoadBalancerOutput {
    /// Load balancer returned by the service.
    #[serde(rename = "LoadBalancer")]
    pub load_balancer: LoadBalancer,
}

crate::wire::product::output::public_fields!(GetLoadBalancerOutput { load_balancer });

/// The result of reading a listener.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetListenerOutput {
    /// Listener returned by the service.
    #[serde(rename = "Listener")]
    pub listener: Listener,
}

crate::wire::product::output::public_fields!(GetListenerOutput { listener });

/// The result of reading a pool.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetPoolOutput {
    /// Pool returned by the service.
    #[serde(rename = "Pool")]
    pub pool: Pool,
}

crate::wire::product::output::public_fields!(GetPoolOutput { pool });

/// The result of reading a health monitor.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetPoolHealthMonitorOutput {
    /// Health monitor returned by the service.
    #[serde(rename = "HealthMonitor")]
    pub health_monitor: HealthMonitor,
}

crate::wire::product::output::public_fields!(GetPoolHealthMonitorOutput { health_monitor });

/// The result of reading a policy.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetPolicyOutput {
    /// Policy returned by the service.
    #[serde(rename = "Policy")]
    pub policy: Policy,
}

crate::wire::product::output::public_fields!(GetPolicyOutput { policy });

/// The result of reading a certificate.
#[derive(Clone, PartialEq, Serialize)]
pub struct GetCertificateOutput {
    /// Certificate returned by the service.
    #[serde(rename = "Certificate")]
    pub certificate: Certificate,
}

crate::wire::product::output::public_fields!(GetCertificateOutput { certificate });
