/// Optional HTTPS endpoint bases for management services.
#[derive(Clone, Default)]
pub struct EndpointOverrides {
    /// VServer endpoint.
    pub vserver: Option<String>,
    /// Load balancer endpoint.
    pub vlb: Option<String>,
    /// Network endpoint.
    pub vnetwork: Option<String>,
    /// Global load balancer endpoint.
    pub global_load_balancer: Option<String>,
    /// DNS endpoint.
    pub dns: Option<String>,
    /// Container registry endpoint.
    pub container_registry: Option<String>,
    /// Portal endpoint.
    pub portal: Option<String>,
    /// IAM sign-in endpoint.
    pub signin: Option<String>,
    /// IAM dashboard endpoint.
    pub dashboard: Option<String>,
    /// IAM token endpoint.
    pub token: Option<String>,
    /// Billing endpoint.
    pub billing: Option<String>,
    /// CDN documentation endpoint.
    pub cdn_docs: Option<String>,
    /// Monitoring endpoint.
    pub monitor: Option<String>,
    /// IAM endpoint.
    pub iam: Option<String>,
    /// Storage endpoint.
    pub storage: Option<String>,
    /// CDN endpoint.
    pub cdn: Option<String>,
    /// Kubernetes endpoint.
    pub vks: Option<String>,
    /// VServer backup endpoint.
    pub vserver_backup: Option<String>,
    /// Backup center endpoint.
    pub backup_center: Option<String>,
    /// Database endpoint.
    pub database: Option<String>,
}

impl std::fmt::Debug for EndpointOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EndpointOverrides([REDACTED])")
    }
}

impl From<EndpointOverrides> for crate::endpoint::EndpointOverrides {
    fn from(value: EndpointOverrides) -> Self {
        Self {
            vserver: value.vserver,
            vlb: value.vlb,
            vnetwork: value.vnetwork,
            global_load_balancer: value.global_load_balancer,
            dns: value.dns,
            container_registry: value.container_registry,
            portal: value.portal,
            signin: value.signin,
            dashboard: value.dashboard,
            token: value.token,
            billing: value.billing,
            cdn_docs: value.cdn_docs,
            monitor: value.monitor,
            iam: value.iam,
            storage: value.storage,
            cdn: value.cdn,
            vks: value.vks,
            vserver_backup: value.vserver_backup,
            backup_center: value.backup_center,
            database: value.database,
        }
    }
}
