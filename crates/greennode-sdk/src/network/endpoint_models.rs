use serde_json::Value;

use crate::model::wire::model;

model! {
    @ true;
    /// Endpoint inventory row and detail fields.
    pub struct Endpoint {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Secondary identifier, if supplied.
        id: String => "id", "ID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Name used by the endpoint inventory.
        endpoint_name: String => "endpointName", "EndpointName";
        /// Internal service URL from the camel-case wire field.
        endpoint_url: String => "endpointUrl", "EndpointURL";
        /// Internal service URL from the snake-case wire field.
        endpoint_url_snake: String => "endpoint_url", "EndpointURLSnake";
        /// Authentication URL from the camel-case wire field.
        endpoint_auth_url: String => "endpointAuthUrl", "EndpointAuthURL";
        /// Authentication URL from the snake-case wire field.
        endpoint_auth_url_snake: String => "endpoint_auth_url", "EndpointAuthURLSnake";
        /// Provider URL for encrypted access.
        endpoint_encrypt_url: String => "endpoint_encrypt_url", "EndpointEncryptURL";
        /// Private address assigned to the endpoint.
        endpoint_ip: String => "endpointIp", "EndpointIP";
        /// Identifier of the backing service.
        endpoint_service_id: String => "endpointServiceId", "EndpointServiceID";
        /// Display label of the backing service.
        endpoint_service_name: String => "endpointServiceName", "EndpointServiceName";
        /// Provider interface kind.
        interface_type: String => "interfaceType", "InterfaceType";
        /// Whether private DNS is enabled.
        enable_private_dns: bool => "enablePrivateDns", "EnablePrivateDNS";
        /// Whether the endpoint DNS name is enabled.
        enable_dns_name: bool => "enableDnsName", "EnableDNSName";
        /// Provisioning state returned by the gateway.
        status: String => "status", "Status";
        /// Creation time in the provider format.
        created_at: String => "createdAt", "CreatedAt";
        /// Last provider sync time in its original format.
        last_sync_time: String => "lastSyncTime", "LastSyncTime";
        /// VPC assigned to this endpoint.
        vpc_id: String => "vpcId", "VPCID";
        /// Subnet assigned to this endpoint.
        subnet_id: String => "subnetId", "SubnetID";
        /// Optional provider-supplied domain data.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        endpoint_domains: Value => "endpointDomains", "EndpointDomains";
        /// Project identifier as returned by this endpoint API.
        project_id: String => "projectId", "ProjectID";
        /// UUID of the owning project.
        project_uuid: String => "projectUuid", "ProjectUUID";
        /// Provider-defined project metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        project: Value => "project", "Project";
        /// Portal user value in the wire type supplied by the gateway.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        portal_user_id: Value => "portalUserId", "PortalUserID";
        /// Region identifier as returned by this endpoint API.
        region_id: String => "regionId", "RegionID";
        /// UUID of the endpoint region.
        region_uuid: String => "regionUuid", "RegionUUID";
        /// Provider-defined region metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        region: Value => "region", "Region";
        /// Identifier shared with the resource service.
        resource_service_id: String => "resourceServiceId", "ResourceServiceID";
        /// Dynamic metadata for the linked resource.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        endpoint_resource: Value => "endpointResource", "EndpointResource";
        /// Supplemental provider metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        endpoint_detail_information: Value => "endpointDetailInformation", "EndpointDetailInformation";
        /// Provider endpoint category, such as Gateway.
        endpoint_type: String => "endpointType", "EndpointType";
        /// Provider-defined catalog category.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        category: Value => "category", "Category";
        /// Provider-defined package data from `apackage`.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        package: Value => "apackage", "Package";
        /// Package selector, which can include an object in the wire response.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        package_id: Value => "packageId", "PackageID";
        /// Display label of the selected package.
        package_name: String => "packageName", "PackageName";
        /// Provider-defined available package data.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        packages: Value => "packages", "Packages";
        /// Provider price record, if present.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        price: Value => "price", "Price";
        /// Monthly price in the original wire type.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        monthly_price: Value => "monthlyPrice", "MonthlyPrice";
        /// Currency code returned with the endpoint.
        currency_unit: String => "currencyUnit", "CurrencyUnit";
        /// SKU used by the billing catalog.
        billing_sku: String => "billingSku", "BillingSKU";
        /// Billing state returned by the gateway.
        billing_status: String => "billingStatus", "BillingStatus";
        /// Project identifier used by the provider backend.
        backend_project_id: String => "backendProjectId", "BackendProjectID";
        /// Free-form endpoint description.
        description: String => "description", "Description";
        /// Provider state for endpoint DNS.
        dns_status: String => "dnsStatus", "DNSStatus";
        /// Dynamic floating address data supplied by the gateway.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        elastic_ips: Value => "elasticIps", "ElasticIPs";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// UUID of the associated route table.
        route_table_uuid: String => "routeTableUuid", "RouteTableUUID";
        /// Display label of the attached interface ACL.
        interface_acl_policy_name: String => "interfaceAclPolicyName", "InterfaceACLPolicyName";
        /// UUID of the attached interface ACL.
        interface_acl_policy_uuid: String => "interfaceAclPolicyUuid", "InterfaceACLPolicyUUID";
        /// Whether the gateway marks this row as the default.
        is_default: bool => "isDefault", "IsDefault";
        /// Provider-defined security group data.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        sec_groups: Value => "secGroups", "SecGroups";
        /// Provider-defined subnet inventory.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        subnets: Value => "subnets", "Subnets";
        /// Provider-defined linked service metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        service: Value => "service", "Service";
        /// Provider-defined subnet metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        subnet: Value => "subnet", "Subnet";
        /// Last update time in the provider format.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Provider version label.
        version: String => "version", "Version";
        /// Provider-defined VPC metadata.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        vpc: Value => "vpc", "VPC";
        /// UUID of the assigned zone.
        zone_uuid: String => "zoneUuid", "ZoneUUID";
    }
}

model! {
    @ true;
    /// Tag attached to a network endpoint.
    pub struct EndpointTag {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Tag key set on the resource.
        key: String => "tagKey", "Key";
        /// Tag value supplied by the resource owner or provider.
        value: String => "tagValue", "Value";
        /// UUID of the tagged resource.
        resource_uuid: String => "resourceUuid", "ResourceUUID";
        /// Provider type of the tagged resource.
        resource_type: String => "resourceType", "ResourceType";
        /// Whether the provider manages the tag.
        system_tag: bool => "systemTag", "SystemTag";
        /// Creation time in the provider format.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update time in the provider format.
        updated_at: String => "updatedAt", "UpdatedAt";
    }
}

model! {
    @ true;
    /// Region metadata returned by vNetwork.
    pub struct VNetworkRegion {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Region code used when selecting a zone.
        code: String => "code", "Code";
        /// Gateway URL advertised by the provider.
        gateway_url: String => "gatewayUrl", "GatewayURL";
        /// Regional vNetwork dashboard URL.
        dashboard_url: String => "vnetworkDashboard", "DashboardURL";
        /// vServer URL advertised in the region metadata.
        vserver_endpoint: String => "vserverEndpoint", "VServerEndpoint";
        /// Load-balancer URL advertised in the region metadata.
        vlb_endpoint: String => "vlbEndpoint", "VLBEndpoint";
        /// Console URL advertised in the region metadata.
        ui_server_endpoint: String => "uiServerEndpoint", "UIServerEndpoint";
        /// vNetwork URL advertised in the region metadata.
        vnetwork_endpoint: String => "vnetworkEndpoint", "VNetworkEndpoint";
        /// DNS URL advertised in the region metadata.
        vdns_endpoint: String => "vdnsEndpoint", "VDNSEndpoint";
    }
}
