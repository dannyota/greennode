use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

use crate::model::wire::{Field, WireField, model};
use crate::wire::product::evidence::Evidence;

model! {
    /// A regional load balancer and its nodes.
    pub struct LoadBalancer {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
        /// Address.
        address: String => "address", "Address";
        /// Private subnet ID.
        private_subnet_id: String => "privateSubnetId", "PrivateSubnetID";
        /// Private subnet CIDR.
        private_subnet_cidr: String => "privateSubnetCidr", "PrivateSubnetCIDR";
        /// Type.
        r#type: String => "type", "Type";
        /// Display type.
        display_type: String => "displayType", "DisplayType";
        /// Network exposure scheme.
        load_balancer_schema: String => "loadBalancerSchema", "LoadBalancerSchema";
        /// Package ID.
        package_id: String => "packageId", "PackageID";
        /// Resource description.
        description: String => "description", "Description";
        /// Location.
        location: String => "location", "Location";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
        /// Status.
        status: String => "status", "Status";
        /// Backend subnet ID.
        backend_subnet_id: String => "backendSubnetId", "BackendSubnetID";
        /// Whether the load balancer is internal.
        internal: bool => "internal", "Internal";
        /// Whether automatic scaling is enabled.
        auto_scalable: bool => "autoScalable", "AutoScalable";
        /// Availability zone identifier.
        zone_id: String => "zoneId", "ZoneID";
        /// Minimum node count.
        min_size: i64 => "minSize", "MinSize";
        /// Maximum node count.
        max_size: i64 => "maxSize", "MaxSize";
        /// Current node count.
        total_nodes: i64 => "totalNodes", "TotalNodes";
        /// Nodes assigned to the load balancer.
        nodes: Vec<Node> => "nodes", "Nodes";
    }
}

model! {
    /// A node belonging to a regional load balancer.
    pub struct Node {
        /// Status.
        status: String => "status", "Status";
        /// Availability zone identifier.
        zone_id: String => "zoneId", "ZoneID";
        /// Zone name.
        zone_name: String => "zoneName", "ZoneName";
        /// Subnet ID.
        subnet_id: String => "subnetId", "SubnetID";
    }
}

model! {
    /// A regional load balancer sizing package.
    pub struct Package {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Type.
        r#type: String => "type", "Type";
        /// Supported connection count.
        connection_number: i64 => "connectionNumber", "ConnectionNumber";
        /// Data transfer allowance.
        data_transfer: i64 => "dataTransfer", "DataTransfer";
        /// Mode.
        mode: String => "mode", "Mode";
        /// Supported load balancer type.
        lb_type: String => "lbType", "LBType";
        /// Display label for the supported load balancer type.
        display_lb_type: String => "displayLbType", "DisplayLBType";
    }
}

model! {
    /// Certificate metadata returned by the regional API.
    pub struct Certificate {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Certificate type.
        certificate_type: String => "certificateType", "CertificateType";
        /// Expired at.
        expired_at: String => "expiredAt", "ExpiredAt";
        /// Imported at.
        imported_at: String => "importedAt", "ImportedAt";
        /// Certificate validity end timestamp.
        not_after: i64 => "notAfter", "NotAfter";
        /// Key algorithm.
        key_algorithm: String => "keyAlgorithm", "KeyAlgorithm";
        /// Serial.
        serial: String => "serial", "Serial";
        /// Subject.
        subject: String => "subject", "Subject";
        /// Domain name.
        domain_name: String => "domainName", "DomainName";
        /// In use.
        in_use: bool => "inUse", "InUse";
        /// Issuer.
        issuer: String => "issuer", "Issuer";
        /// Signature algorithm.
        signature_algorithm: String => "signatureAlgorithm", "SignatureAlgorithm";
        /// Certificate validity start timestamp.
        not_before: i64 => "notBefore", "NotBefore";
        /// Subject alternative names.
        subject_alternative_names: Vec<String> => "subjectAlternativeNames", "SubjectAlternativeNames";
    }
}

model! {
    /// A tag assigned to a regional load balancer.
    pub struct Tag {
        /// Key.
        key: String => "key", "Key";
        /// Value.
        value: String => "value", "Value";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Whether the tag is managed by the service.
        system_tag: bool => "systemTag", "SystemTag";
    }
}

model! {
    /// A header added by a listener.
    pub struct ListenerInsertHeader {
        /// Header name.
        header_name: String => "headerName", "HeaderName";
        /// Header value.
        header_value: String => "headerValue", "HeaderValue";
    }
}

model! {
    /// A listener attached to a regional load balancer.
    pub struct Listener {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Resource description.
        description: String => "description", "Description";
        /// Protocol.
        protocol: String => "protocol", "Protocol";
        /// Protocol port.
        protocol_port: i64 => "protocolPort", "ProtocolPort";
        /// Maximum simultaneous connections.
        connection_limit: i64 => "connectionLimit", "ConnectionLimit";
        /// Default pool ID.
        default_pool_id: String => "defaultPoolId", "DefaultPoolID";
        /// Default pool name.
        default_pool_name: String => "defaultPoolName", "DefaultPoolName";
        /// Client connection timeout.
        timeout_client: i64 => "timeoutClient", "TimeoutClient";
        /// Backend member timeout.
        timeout_member: i64 => "timeoutMember", "TimeoutMember";
        /// Timeout while connecting to a backend.
        timeout_connection: i64 => "timeoutConnection", "TimeoutConnection";
        /// CIDR ranges allowed by the listener.
        allowed_cidrs: String => "allowedCidrs", "AllowedCIDRs";
        /// Certificate authority identifiers.
        certificate_authorities: Vec<String> => "certificateAuthorities", "CertificateAuthorities";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Default certificate authority identifier.
        default_certificate_authority: Option<String> => "defaultCertificateAuthority", "DefaultCertificateAuthority";
        /// Client certificate authentication.
        client_certificate_authentication: Option<String> => "clientCertificateAuthentication", "ClientCertificateAuthentication";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
        /// Insert headers.
        insert_headers: Vec<ListenerInsertHeader> => "insertHeaders", "InsertHeaders";
    }
}

model! {
    /// A regional load balancer pool and its configuration.
    pub struct Pool {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Protocol.
        protocol: String => "protocol", "Protocol";
        /// Resource description.
        description: String => "description", "Description";
        /// Load balance method.
        load_balance_method: String => "loadBalanceMethod", "LoadBalanceMethod";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
        /// Stickiness.
        stickiness: bool => "stickiness", "Stickiness";
        /// Whether backend traffic uses TLS.
        tls_encryption: bool => "tlsEncryption", "TLSEncryption";
        /// Members.
        members: Vec<PoolMember> => "members", "Members";
        /// The health monitor configured for a regional pool.
        health_monitor: Option<HealthMonitor> => "healthMonitor", "HealthMonitor";
    }
}

model! {
    /// A backend member of a regional pool.
    pub struct PoolMember {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Address.
        address: String => "address", "Address";
        /// Protocol port.
        protocol_port: i64 => "protocolPort", "ProtocolPort";
        /// Weight.
        weight: i64 => "weight", "Weight";
        /// Monitor port.
        monitor_port: i64 => "monitorPort", "MonitorPort";
        /// Subnet ID.
        subnet_id: String => "subnetId", "SubnetID";
        /// Resource name.
        name: String => "name", "Name";
        /// Pool ID.
        pool_id: String => "poolId", "PoolID";
        /// Type create.
        type_create: String => "typeCreate", "TypeCreate";
        /// Backup.
        backup: bool => "backup", "Backup";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updateAt", "UpdatedAt";
        /// Identity that added the member.
        created_by: String => "createdBy", "CreatedBy";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
    }
}

model! {
    /// The health monitor configured for a regional pool.
    pub struct HealthMonitor {
        /// Timeout.
        timeout: i64 => "timeout", "Timeout";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Domain name.
        domain_name: Option<String> => "domainName", "DomainName";
        /// HTTP version used for health checks.
        http_version: Option<String> => "httpVersion", "HTTPVersion";
        /// Health check protocol.
        health_check_protocol: String => "healthCheckProtocol", "HealthCheckProtocol";
        /// Interval.
        interval: i64 => "interval", "Interval";
        /// Healthy threshold.
        healthy_threshold: i64 => "healthyThreshold", "HealthyThreshold";
        /// Unhealthy threshold.
        unhealthy_threshold: i64 => "unhealthyThreshold", "UnhealthyThreshold";
        /// Health check method.
        health_check_method: Option<String> => "healthCheckMethod", "HealthCheckMethod";
        /// Health check path.
        health_check_path: Option<String> => "healthCheckPath", "HealthCheckPath";
        /// Success code.
        success_code: Option<String> => "successCode", "SuccessCode";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
    }
}

model! {
    /// A layer 7 policy attached to a listener.
    pub struct Policy {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Resource name.
        name: String => "name", "Name";
        /// Resource description.
        description: String => "description", "Description";
        /// Redirect pool ID.
        redirect_pool_id: String => "redirectPoolId", "RedirectPoolID";
        /// Redirect pool name.
        redirect_pool_name: String => "redirectPoolName", "RedirectPoolName";
        /// Action.
        action: String => "action", "Action";
        /// Redirect URL.
        redirect_url: String => "redirectUrl", "RedirectURL";
        /// HTTP status used for redirection.
        redirect_http_code: i64 => "redirectHttpCode", "RedirectHTTPCode";
        /// Whether to retain the query string on redirect.
        keep_query_string: bool => "keepQueryString", "KeepQueryString";
        /// Position.
        position: i64 => "position", "Position";
        /// Rules that match traffic for the policy.
        l7_rules: Vec<L7Rule> => "l7Rules", "L7Rules";
        /// Display status.
        display_status: String => "displayStatus", "DisplayStatus";
        /// Creation timestamp.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update timestamp.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Progress status.
        progress_status: String => "progressStatus", "ProgressStatus";
    }
}

model! {
    /// A match rule belonging to a layer 7 policy.
    pub struct L7Rule {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Compare type.
        compare_type: String => "compareType", "CompareType";
        /// Rule value.
        rule_value: String => "ruleValue", "RuleValue";
        /// Rule type.
        rule_type: String => "ruleType", "RuleType";
        /// Provisioning status.
        provisioning_status: String => "provisioningStatus", "ProvisioningStatus";
        /// Operating status.
        operating_status: String => "operatingStatus", "OperatingStatus";
    }
}

model! {
    struct LoadBalancerZone {
        zone: ZoneRef => "zone", "Zone";
    }
}

model! {
    struct ZoneRef {
        uuid: String => "uuid", "UUID";
    }
}

#[derive(Default)]
pub(super) struct ZonedLoadBalancer(pub(super) LoadBalancer);

impl<'de> Deserialize<'de> for ZonedLoadBalancer {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(decoder)?.get();
        let Field(mut load_balancer) =
            serde_json::from_str::<Field<LoadBalancer>>(raw).map_err(serde::de::Error::custom)?;
        let Field(LoadBalancerZone { zone }) = serde_json::from_str::<Field<LoadBalancerZone>>(raw)
            .map_err(serde::de::Error::custom)?;
        if !zone.uuid.is_empty() {
            load_balancer.zone_id = zone.uuid;
        }
        Ok(Self(load_balancer))
    }
}

impl WireField for ZonedLoadBalancer {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.0.redact(evidence)
    }
}
