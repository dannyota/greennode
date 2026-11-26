use std::net::IpAddr;

use super::wire::model;

model! {
    /// A vServer instance and its embedded resources.
    pub struct Server {
        /// Boot volume id.
        boot_volume_id: String => "bootVolumeId", "BootVolumeID";
        /// Created at.
        created_at: String => "createdAt", "CreatedAt";
        /// Resource description.
        description: String => "description", "Description";
        /// Encryption volume.
        encryption_volume: bool => "encryptionVolume", "EncryptionVolume";
        /// Enable log.
        enable_log: bool => "enableLog", "EnableLog";
        /// Enable metric.
        enable_metric: bool => "enableMetric", "EnableMetric";
        /// Licence.
        licence: bool => "licence", "Licence";
        /// License key reported for the image or server.
        license_key: String => "licenseKey", "LicenseKey";
        /// Location.
        location: String => "location", "Location";
        /// Metadata.
        metadata: String => "metadata", "Metadata";
        /// Migrate state.
        migrate_state: String => "migrateState", "MigrateState";
        /// Migration status.
        migration_status: String => "migrationStatus", "MigrationStatus";
        /// Display name.
        name: String => "name", "Name";
        /// Product.
        product: String => "product", "Product";
        /// Server group id.
        #[serde(serialize_with = "super::wire::serialize_dynamic")]
        server_group_id: serde_json::Value => "serverGroupId", "ServerGroupID";
        /// Server group name.
        server_group_name: String => "serverGroupName", "ServerGroupName";
        /// Ssh key name.
        ssh_key_name: String => "sshKeyName", "SSHKeyName";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Stop before migrate.
        stop_before_migrate: bool => "stopBeforeMigrate", "StopBeforeMigrate";
        /// User.
        user: String => "user", "User";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Image.
        image: Image => "image", "Image";
        /// Flavor.
        flavor: Flavor => "flavor", "Flavor";
        /// Security groups.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        security_groups: Option<Vec<ServerSecgroup>> => "secGroups", "SecurityGroups";
        /// External interfaces.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        external_interfaces: Option<Vec<NetworkInterface>> => "externalInterfaces", "ExternalInterfaces";
        /// Internal interfaces.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        internal_interfaces: Option<Vec<NetworkInterface>> => "internalInterfaces", "InternalInterfaces";
        /// Zone id.
        zone_id: String => "zoneId", "ZoneID";
        /// Zone.
        zone: Zone => "zone", "Zone";
        /// App license.
        #[serde(serialize_with = "super::wire::serialize_dynamic")]
        app_license: serde_json::Value => "appLicense", "AppLicense";
        /// App license name.
        app_license_name: String => "appLicenseName", "AppLicenseName";
        /// App package version name.
        app_package_version_name: String => "appPackageVersionName", "AppPackageVersionName";
        /// Default tag i ds.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        default_tag_ids: Option<Vec<String>> => "defaultTagIds", "DefaultTagIDs";
        /// Flavor zone id.
        flavor_zone_id: String => "flavorZoneId", "FlavorZoneID";
        /// Flavor zones.
        #[serde(serialize_with = "super::wire::serialize_dynamic")]
        flavor_zones: serde_json::Value => "flavorZones", "FlavorZones";
        /// Gpu memory.
        #[serde(serialize_with = "super::wire::serialize_dynamic")]
        gpu_memory: serde_json::Value => "gpuMemory", "GPUMemory";
        /// Host group id.
        host_group_id: String => "hostGroupId", "HostGroupID";
    } derive {
        /// Canonical unambiguous public address from the internal interfaces.
        public_ip: String => "PublicIP", |row: &Server| public_ip(row.internal_interfaces.as_deref().unwrap_or_default());
    }
}

model! {
    /// An interface attached to a server.
    pub struct NetworkInterface {
        /// Created at.
        created_at: String => "createdAt", "CreatedAt";
        /// Fixed ip.
        fixed_ip: String => "fixedIp", "FixedIP";
        /// Floating ip.
        floating_ip: String => "floatingIp", "FloatingIP";
        /// Floating ipid.
        floating_ipid: String => "floatingIpId", "FloatingIPID";
        /// Interface type.
        interface_type: String => "interfaceType", "InterfaceType";
        /// Mac.
        mac: String => "mac", "MAC";
        /// Network uuid.
        network_uuid: String => "networkUuid", "NetworkUUID";
        /// Port uuid.
        port_uuid: String => "portUuid", "PortUUID";
        /// Product.
        product: String => "product", "Product";
        /// Server uuid.
        server_uuid: String => "serverUuid", "ServerUUID";
        /// Service-reported status.
        status: String => "status", "Status";
        /// Subnet uuid.
        subnet_uuid: String => "subnetUuid", "SubnetUUID";
        /// Type.
        r#type: String => "type", "Type";
        /// Updated at.
        updated_at: String => "updatedAt", "UpdatedAt";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
    }
}

model! {
    /// A server size from an instance or flavor catalog.
    pub struct Flavor {
        /// Bandwidth.
        bandwidth: i64 => "bandwidth", "Bandwidth";
        /// Bandwidth unit.
        bandwidth_unit: String => "bandwidthUnit", "BandwidthUnit";
        /// Number of virtual CPUs.
        cpu: i64 => "cpu", "CPU";
        /// Cpu platform description.
        cpu_platform_description: String => "cpuPlatformDescription", "CPUPlatformDescription";
        /// Flavor id.
        flavor_id: String => "flavorId", "FlavorID";
        /// Flavor zone id.
        flavor_zone_id: String => "flavorZoneId", "FlavorZoneID";
        /// Number of GPUs.
        gpu: i64 => "gpu", "GPU";
        /// Group.
        group: String => "group", "Group";
        /// Is sold out.
        is_sold_out: bool => "isSoldOut", "IsSoldOut";
        /// Memory reported by the service.
        memory: i64 => "memory", "Memory";
        /// Metadata.
        metadata: String => "metaData", "Metadata";
        /// Display name.
        name: String => "name", "Name";
        /// Remaining v ms.
        remaining_vms: i64 => "remainingVms", "RemainingVMs";
        /// Zone id.
        zone_id: String => "zoneId", "ZoneID";
    }
}

model! {
    /// The image embedded in a server.
    pub struct Image {
        /// Flavor zone i ds.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        flavor_zone_ids: Option<Vec<String>> => "flavorZoneIds", "FlavorZoneIDs";
        /// Identifier.
        id: String => "id", "ID";
        /// Image type.
        image_type: String => "imageType", "ImageType";
        /// Image version.
        image_version: String => "imageVersion", "ImageVersion";
        /// Licence.
        licence: bool => "licence", "Licence";
        /// Package limit.
        package_limit: PackageLimit => "packageLimit", "PackageLimit";
    }
}

model! {
    /// Resource minimums of an image package.
    pub struct PackageLimit {
        /// Number of virtual CPUs.
        cpu: i64 => "cpu", "CPU";
        /// Disk size.
        disk_size: i64 => "diskSize", "DiskSize";
        /// Memory reported by the service.
        memory: i64 => "memory", "Memory";
    }
}

model! {
    /// A security group attached to a server.
    pub struct ServerSecgroup {
        /// Display name.
        name: String => "name", "Name";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
    }
}

model! {
    /// The network zone embedded in a regional resource.
    pub struct Zone {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Display name.
        name: String => "name", "Name";
        /// Zone type.
        zone_type: String => "zoneType", "ZoneType";
        /// Is default.
        is_default: bool => "isDefault", "IsDefault";
        /// Resource description.
        description: String => "description", "Description";
        /// Is enabled.
        is_enabled: bool => "isEnabled", "IsEnabled";
        /// Open stack zone.
        openstack_zone: String => "openstackZone", "OpenStackZone";
        /// Ip ranges.
        #[serde(serialize_with = "super::wire::serialize_slice")]
        ip_ranges: Option<Vec<String>> => "ipRanges", "IPRanges";
        /// Server count.
        server_count: i64 => "serverCount", "ServerCount";
        /// Volume count.
        volume_count: i64 => "volumeCount", "VolumeCount";
    }
}

impl Server {
    /// Returns whether the server has ACTIVE status.
    pub fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("ACTIVE")
    }

    /// Returns whether the reported status permits deletion.
    pub fn can_delete(&self) -> bool {
        ["ACTIVE", "ERROR", "STOPPED"]
            .iter()
            .any(|status| self.status.eq_ignore_ascii_case(status))
    }
}

fn public_ip(interfaces: &[NetworkInterface]) -> String {
    let mut public = None;
    for interface in interfaces.iter().filter(|nic| !nic.floating_ip.is_empty()) {
        let Ok(address) = interface.floating_ip.parse::<IpAddr>() else {
            return String::new();
        };
        let address = address.to_canonical();
        let valid = match address {
            IpAddr::V4(ip) => {
                !ip.is_private()
                    && !ip.is_unspecified()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_multicast()
                    && !ip.is_broadcast()
            }
            IpAddr::V6(ip) => {
                !ip.is_unique_local()
                    && !ip.is_unspecified()
                    && !ip.is_loopback()
                    && !ip.is_unicast_link_local()
                    && !ip.is_multicast()
            }
        };
        if !valid || public.is_some_and(|previous| previous != address) {
            return String::new();
        }
        public = Some(address);
    }
    public
        .map(|address| address.to_string())
        .unwrap_or_default()
}
