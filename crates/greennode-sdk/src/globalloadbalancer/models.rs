use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};

use super::wire::{WireField, deserialize_field, serialize_detail};

macro_rules! model {
    ($(#[$model_meta:meta])* pub struct $model:ident {
        $($(#[$field_meta:meta])* $field:ident: $type:ty => $wire:literal, $go:literal;)*
    }) => {
        $(#[$model_meta])*
        #[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
        #[serde(default)]
        pub struct $model {
            $(
                $(#[$field_meta])*
                #[serde(rename(serialize = $go, deserialize = $wire), deserialize_with = "deserialize_field")]
                pub $field: $type,
            )*
        }

        impl WireField for $model {
            const SHAPE: Shape<'static> = Shape::Object(&[
                $(ShapeField { name: $wire, shape: <$type as WireField>::SHAPE },)*
            ]);

            fn redact(&mut self, evidence: &Evidence) -> bool {
                $((if crate::wire::product::evidence::identifier_key($wire) {
                    self.$field.redact_identifier(evidence)
                } else {
                    self.$field.redact(evidence)
                }) &&)* true
            }
        }
    };
}

model! {
    /// A global load balancer package.
    pub struct Package {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// English description.
        description_en: String => "descriptionEn", "DescriptionEn";
        /// Whether the package is enabled.
        enabled: bool => "enabled", "Enabled";
        /// Base package SKU.
        base_sku: String => "baseSku", "BaseSKU";
        /// SKU used for additional connections.
        connection_sku: String => "connectionSku", "ConnectionSKU";
        /// Included connection rate.
        base_connection_rate: i64 => "baseConnectionRate", "BaseConnectionRate";
        /// Included domestic traffic.
        base_domestic_traffic_total: i64 => "baseDomesticTrafficTotal", "BaseDomesticTrafficTotal";
        /// Included non-domestic traffic.
        base_non_domestic_traffic_total: i64 => "baseNonDomesticTrafficTotal", "BaseNonDomesticTrafficTotal";
        /// Domestic traffic SKU.
        domestic_traffic_sku: String => "domesticTrafficSku", "DomesticTrafficSKU";
        /// Non-domestic traffic SKU.
        non_domestic_traffic_sku: String => "nonDomesticTrafficSku", "NonDomesticTrafficSKU";
        /// Additional package attributes.
        #[serde(serialize_with = "serialize_detail")]
        detail: Value => "detail", "Detail";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Regional load balancer packages included in this package.
        vlb_packages: Vec<RegionalPackage> => "vlbPackages", "VLBPackages";
    }
}

model! {
    /// A regional load balancer package associated with a global package.
    pub struct RegionalPackage {
        /// Identifier.
        id: i64 => "id", "ID";
        /// Global load balancer package identifier.
        glb_package_id: String => "glb_package_id", "GLBPackageID";
        /// Region identifier.
        region: String => "region", "Region";
        /// Regional load balancer package identifier.
        vlb_package_id: String => "vlb_package_id", "VLBPackageID";
        /// Creation timestamp.
        created_at: String => "created_at", "CreatedAt";
    }
}

model! {
    /// A region available to the global load balancer service.
    pub struct Region {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Reported service status.
        status: String => "status", "Status";
        /// Regional vServer endpoint.
        #[serde(alias = "vServerEndpoint")]
        vserver_endpoint: String => "vserverEndpoint", "VServerEndpoint";
        /// Regional load balancer endpoint.
        vlb_endpoint: String => "vlbEndpoint", "VLBEndpoint";
        /// Regional console endpoint.
        ui_server_endpoint: String => "uiServerEndpoint", "UIServerEndpoint";
    }
}

model! {
    /// A global load balancer.
    pub struct LoadBalancer {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Reported service status.
        status: String => "status", "Status";
        /// Associated package.
        package: String => "package", "Package";
        /// Service-reported type.
        r#type: String => "type", "Type";
        /// Owning user identifier.
        user_id: i64 => "userId", "UserID";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: String => "deletedAt", "DeletedAt";
        /// Assigned virtual IP addresses.
        vips: Vec<Vip> => "vips", "VIPs";
        /// Associated DNS names.
        domains: Vec<Domain> => "domains", "Domains";
    }
}

model! {
    /// A virtual IP address assigned to a global load balancer.
    pub struct Vip {
        /// Identifier.
        id: i64 => "id", "ID";
        /// IP address or hostname.
        address: String => "address", "Address";
        /// Reported service status.
        status: String => "status", "Status";
        /// Region identifier.
        region: String => "region", "Region";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: String => "deletedAt", "DeletedAt";
    }
}

model! {
    /// A DNS name associated with a global load balancer.
    pub struct Domain {
        /// Identifier.
        id: i64 => "id", "ID";
        /// Associated hostname.
        hostname: String => "hostname", "Hostname";
        /// Reported service status.
        status: String => "status", "Status";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// DNS hosted zone identifier.
        dns_hosted_zone_id: String => "dnsHostedZoneId", "DNSHostedZoneID";
        /// DNS server identifier.
        dns_server_id: String => "dnsServerId", "DNSServerID";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: String => "deletedAt", "DeletedAt";
    }
}

model! {
    /// A backend pool in a global load balancer.
    pub struct Pool {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Backend selection algorithm.
        algorithm: String => "algorithm", "Algorithm";
        /// Session affinity setting.
        sticky_session: Option<String> => "stickySession", "StickySession";
        /// Backend TLS setting.
        tls_enabled: Option<String> => "tlsEnabled", "TLSEnabled";
        /// Network protocol.
        protocol: String => "protocol", "Protocol";
        /// Reported service status.
        status: String => "status", "Status";
        /// Configured health monitor.
        health: Option<PoolHealthMonitor> => "health", "Health";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
    }
}

model! {
    /// A pool health check and its configuration.
    pub struct PoolHealthMonitor {
        /// Identifier.
        id: String => "id", "ID";
        /// Associated global pool identifier.
        global_pool_id: String => "globalPoolId", "GlobalPoolID";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Network protocol.
        protocol: String => "protocol", "Protocol";
        /// HTTP health check path.
        path: Option<String> => "path", "Path";
        /// Health check timeout.
        timeout: i64 => "timeout", "Timeout";
        /// Interval between health checks.
        interval_time: i64 => "intervalTime", "IntervalTime";
        /// Successful checks required to mark a backend healthy.
        healthy_threshold: i64 => "healthyThreshold", "HealthyThreshold";
        /// Failed checks required to mark a backend unhealthy.
        unhealthy_threshold: i64 => "unhealthyThreshold", "UnhealthyThreshold";
        /// HTTP version used for checks.
        http_version: Option<String> => "httpVersion", "HTTPVersion";
        /// HTTP method used for checks.
        http_method: Option<String> => "httpMethod", "HTTPMethod";
        /// Domain name used for checks.
        domain_name: Option<String> => "domainName", "DomainName";
        /// HTTP response codes considered healthy.
        success_code: Option<String> => "successCode", "SuccessCode";
        /// Reported service status.
        status: String => "status", "Status";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
    }
}

model! {
    /// A group of backend members in a global pool.
    pub struct PoolMember {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Region identifier.
        region: String => "region", "Region";
        /// Associated global pool identifier.
        global_pool_id: String => "globalPoolId", "GlobalPoolID";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Traffic allocation to the group.
        traffic_dial: i64 => "trafficDial", "TrafficDial";
        /// Backend VPC identifier.
        vpc_id: String => "vpcId", "VPCID";
        /// Service-reported type.
        r#type: String => "type", "Type";
        /// Reported service status.
        status: String => "status", "Status";
        /// Backends in this group.
        members: Vec<PoolMemberDetail> => "members", "Members";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
    }
}

model! {
    /// One backend in a global pool member group.
    pub struct PoolMemberDetail {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Parent member group identifier.
        global_pool_member_id: String => "globalPoolMemberId", "GlobalPoolMemberID";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Backend subnet identifier.
        subnet_id: String => "subnetId", "SubnetID";
        /// IP address or hostname.
        address: String => "address", "Address";
        /// Relative backend weight.
        weight: i64 => "weight", "Weight";
        /// Service port number.
        port: i64 => "port", "Port";
        /// Health check port number.
        monitor_port: i64 => "monitorPort", "MonitorPort";
        /// Whether the backend serves as a backup.
        backup_role: bool => "backupRole", "BackupRole";
        /// Reported service status.
        status: String => "status", "Status";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
    }
}

model! {
    /// A listener attached to a global load balancer.
    pub struct Listener {
        /// Identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description supplied by the service.
        description: String => "description", "Description";
        /// Network protocol.
        protocol: String => "protocol", "Protocol";
        /// Service port number.
        port: i64 => "port", "Port";
        /// Parent global load balancer identifier.
        global_load_balancer_id: String => "globalLoadBalancerId", "GlobalLoadBalancerID";
        /// Associated global pool identifier.
        global_pool_id: String => "globalPoolId", "GlobalPoolID";
        /// Client connection timeout.
        timeout_client: i64 => "timeoutClient", "TimeoutClient";
        /// Backend connection timeout.
        timeout_member: i64 => "timeoutMember", "TimeoutMember";
        /// Connection establishment timeout.
        timeout_connection: i64 => "timeoutConnection", "TimeoutConnection";
        /// Source CIDRs permitted to connect.
        allowed_cidrs: String => "allowedCidrs", "AllowedCIDRs";
        /// Listener header configuration.
        headers: Option<String> => "headers", "Headers";
        /// Reported service status.
        status: String => "status", "Status";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Deletion timestamp.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
    }
}

model! {
    /// One measurement in a load balancer usage history.
    pub struct UsageHistory {
        /// Measurement timestamp.
        timestamp: String => "timestamp", "Timestamp";
        /// Measured usage.
        value: f64 => "value", "Value";
        /// Service-reported type.
        r#type: String => "type", "Type";
    }
}

model! {
    /// One page of global load balancers.
    pub struct ListLoadBalancersOutput {
        /// Load balancers in this page.
        items: Vec<LoadBalancer> => "items", "Items";
        /// Offset reported in this page.
        offset: i64 => "offset", "Offset";
        /// Page size reported by the service.
        limit: i64 => "limit", "Limit";
        /// Total number of load balancers reported by the service.
        total: i64 => "total", "Total";
    }
}

model! {
    /// Usage measurements and the filters reported by the service.
    pub struct ListUsageHistoriesOutput {
        /// Type of usage measured.
        r#type: String => "type", "Type";
        /// Start of the reported time range.
        from: String => "from", "From";
        /// End of the reported time range.
        to: String => "to", "To";
        /// Reported usage measurements.
        items: Vec<UsageHistory> => "items", "Items";
    }
}
