use crate::model::wire::model;

model! {
    /// Public NAT instance returned by the provider.
    pub struct NatInstance {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label of the Public NAT.
        nat_name: String => "natName", "NATName";
        /// Provisioning state returned by the gateway.
        status: String => "status", "Status";
        /// Creation time in the provider format.
        created_at: String => "createdAt", "CreatedAt";
        /// Last update time in the provider format.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// UUID of the project that owns the NAT.
        project_uuid: String => "projectUuid", "ProjectUUID";
        /// UUID of the regional vNetwork zone.
        zone_uuid: String => "zoneUuid", "ZoneUUID";
        /// Gateway address when provisioned.
        nat_gateway_ip: Option<String> => "natGatewayIp", "NATGatewayIP";
        /// Assigned public address, when present.
        public_ip: Option<String> => "publicIp", "PublicIP";
        /// Deletion time when supplied by the provider.
        deleted_at: Option<String> => "deletedAt", "DeletedAt";
        /// Billing state returned by the gateway.
        billing_status: Option<String> => "billingStatus", "BillingStatus";
        /// Provisioned package and image metadata.
        nat_package: NatPackage => "natPackage", "NATPackage";
        /// VPC metadata returned with the NAT.
        vpc: NatVpc => "vpc", "VPC";
    }
}

model! {
    /// Public NAT package returned by the provider.
    pub struct NatPackage {
        /// Secondary identifier, if supplied.
        id: String => "id", "ID";
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Creation time in the provider format.
        created_at: String => "createdAt", "CreatedAt";
        /// Secondary catalog package identifier.
        package_id: String => "packageId", "PackageID";
        /// Whether the catalog marks this package as the default.
        default: bool => "default", "Default";
        /// Provider image used by this package.
        image: NatImage => "image", "Image";
    }
}

model! {
    /// Public NAT image returned by the provider.
    pub struct NatImage {
        /// Secondary identifier, if supplied.
        id: String => "id", "ID";
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Image family in the catalog.
        image_type: String => "imageType", "ImageType";
        /// Version label supplied by the catalog.
        image_version: String => "imageVersion", "ImageVersion";
        /// License label returned for the image.
        licence: String => "licence", "Licence";
        /// Identifiers of zones carrying this flavor.
        flavor_zone_ids: Vec<String> => "flavorZoneIds", "FlavorZoneIDs";
        /// Capacity limits reported by the image catalog.
        package_limit: NatPackageLimit => "packageLimit", "PackageLimit";
    }
}

model! {
    /// Public NAT packagelimit returned by the provider.
    pub struct NatPackageLimit {
        /// Raw CPU limit reported by the provider.
        cpu: i64 => "cpu", "CPU";
        /// Raw memory limit in the provider’s units.
        memory: i64 => "memory", "Memory";
        /// Raw disk limit in the provider’s units.
        disk_size: i64 => "diskSize", "DiskSize";
    }
}

model! {
    /// Public NAT vpc returned by the provider.
    pub struct NatVpc {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Address range in CIDR notation.
        cidr: String => "cidr", "CIDR";
        /// Provisioning state returned by the gateway.
        status: String => "status", "Status";
        /// Region identifier as returned by this endpoint API.
        region_id: String => "regionId", "RegionID";
        /// Project identifier as returned by this endpoint API.
        project_id: String => "projectId", "ProjectID";
        /// Last provider sync time in its original format.
        last_sync_time: String => "lastSyncTime", "LastSyncTime";
        /// Provider state for endpoint DNS.
        dns_status: String => "dnsStatus", "DNSStatus";
    }
}

model! {
    /// Public NAT availabilityzone returned by the provider.
    pub struct NatAvailabilityZone {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Zone type as returned by the provider.
        zone_type: String => "zoneType", "ZoneType";
        /// Whether the zone currently accepts placements.
        is_enabled: bool => "isEnabled", "IsEnabled";
        /// Whether the gateway marks this row as the default.
        is_default: bool => "isDefault", "IsDefault";
        /// Free-form endpoint description.
        description: String => "description", "Description";
    }
}

model! {
    /// Public NAT offerprice returned by the provider.
    pub struct NatOfferPrice {
        /// Provider price after applying the offered discount.
        optimum_price: f64 => "optimumPrice", "OptimumPrice";
        /// Provider price before discounts.
        original_price: f64 => "originalPrice", "OriginalPrice";
        /// Discount amount supplied by the catalog.
        discount_price: f64 => "discountPrice", "DiscountPrice";
        /// Discount percentage supplied by the catalog.
        discount_percent: f64 => "discountPercent", "DiscountPercent";
    }
}

model! {
    /// Public NAT packageoffer returned by the provider.
    pub struct NatPackageOffer {
        /// Provider identifier for this row.
        uuid: String => "uuid", "UUID";
        /// Display label when returned.
        name: String => "name", "Name";
        /// Secondary catalog package identifier.
        package_id: String => "packageId", "PackageID";
        /// Identifier shared with the resource service.
        resource_service_id: String => "resourceServiceId", "ResourceServiceID";
        /// SKU used in the billing catalog.
        billing_sku: String => "billingSku", "BillingSKU";
        /// Provider name of the backing service.
        service_name: String => "serviceName", "ServiceName";
        /// Free-form endpoint description.
        description: Option<String> => "description", "Description";
        /// Currency code returned with the endpoint.
        currency_unit: String => "currencyUnit", "CurrencyUnit";
        /// Creation time in the provider format.
        created_at: String => "createdAt", "CreatedAt";
        /// Whether the gateway marks this row as the default.
        is_default: bool => "isDefault", "IsDefault";
        /// Catalog amount in the specified currency per month.
        monthly_price: f64 => "monthlyPrice", "MonthlyPrice";
        /// Price breakdown for the catalog offer.
        price: NatOfferPrice => "price", "Price";
    }
}
