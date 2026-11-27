use crate::wire::strict_json::FieldNames;
use serde::{Serialize, Serializer};
use serde_json::{Value, value::RawValue};

use super::wire::{FromWire, Object, Redact, ShapeResult};
use crate::wire::product::evidence::Evidence;

fn slice_or_empty<S: Serializer>(
    value: &Option<Vec<String>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.as_deref().unwrap_or_default().serialize(serializer)
}

macro_rules! row {
    ($(#[$doc:meta])* $name:ident { $($(#[$field_doc:meta])* $field:ident : $ty:ty = $key:literal, $wire:literal, $read:ident;)* }) => {
        $(#[$doc])*
        #[derive(Clone, Default, PartialEq, Serialize)]
        #[serde(remote = "Self")]
        pub struct $name {
            $($(#[$field_doc])* #[serde(rename = $key)] pub $field: $ty,)*
        }

        crate::error::debug_record!($name { $($field),* });

        impl crate::wire::product::output::PublicOutput for $name {
            fn public_reflects(&self, scan: &crate::wire::product::output::FieldScan<'_>, identifier: bool) -> bool {
                $(scan.field(&self.$field, identifier, $wire, $key) ||)* false
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, output: S) -> Result<S::Ok, S::Error> {
                $(if crate::wire::product::identifier::declared($wire, $key) {
                    crate::wire::product::identifier::check::<_, S::Error>(&self.$field)?;
                })*
                Self::serialize(self, output)
            }
        }
        impl FromWire for $name {
            fn from_wire(raw: &RawValue, names: FieldNames) -> ShapeResult<Self> {
                let fields = Object::parse(raw, names)?;
                Ok(Self { $($field: fields.$read($wire)?,)* })
            }
        }
        impl Redact for $name {
            fn redact(&mut self, evidence: &Evidence) {
                $(if crate::wire::product::identifier::declared($wire, $key) {
                    self.$field.redact_identifier(evidence);
                } else {
                    self.$field.redact(evidence);
                })*
            }
        }
    };
}

row! {
    /// Volume returned by the volume service.
    Volume {
        /// Provider-assigned UUID.
        uuid: String = "UUID", "uuid", value;
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Owning project identifier.
        project_id: String = "ProjectID", "projectId", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Resource description supplied by the provider.
        description: String = "Description", "description", value;
        /// Provisioned size reported by the provider.
        size: u64 = "Size", "size", value;
        /// Current provider state used for settlement.
        status: String = "Status", "status", value;
        /// Provider text describing the current state.
        status_message: String = "StatusMessage", "statusMessage", value;
        /// Identifier of the volume type associated with the resource.
        volume_type_id: String = "VolumeTypeID", "volumeTypeId", value;
        /// Provider-specific volume type details.
        volume_type: Value = "VolumeType", "volumeType", value;
        /// Display name of the volume-type zone.
        volume_type_zone_name: String = "VolumeTypeZoneName", "volumeTypeZoneName", value;
        /// Provisioned I/O operations per second.
        iops: String = "IOPS", "iops", value;
        /// Provisioned throughput in the provider's units.
        throughput: Value = "Throughput", "throughPut", value;
        /// Identifier of a server attached to the volume.
        server_id: String = "ServerID", "serverId", value;
        /// Display names of attached servers.
        #[serde(serialize_with = "slice_or_empty")]
        server_name_list: Option<Vec<String>> = "ServerNameList", "serverNameList", optional;
        /// Creation time in the provider's format.
        created_at: String = "CreatedAt", "createdAt", value;
        /// Last update time in the provider's format.
        updated_at: Option<String> = "UpdatedAt", "updatedAt", optional;
        /// Whether this volume can boot a server.
        bootable: bool = "Bootable", "bootable", value;
        /// Encryption mode returned with the resource.
        encryption_type: Option<String> = "EncryptionType", "encryptionType", optional;
        /// Provider encryption key identifier, when present.
        encryption_key_id: String = "EncryptionKeyID", "encryptionKeyId", value;
        /// Attachment position in the server's boot order.
        boot_index: i64 = "BootIndex", "bootIndex", value;
        /// Whether more than one server can attach to the volume.
        multi_attach: bool = "MultiAttach", "multiAttach", value;
        /// Identifiers of attached servers.
        #[serde(serialize_with = "slice_or_empty")]
        server_id_list: Option<Vec<String>> = "ServerIDList", "serverIdList", optional;
        /// Provider location label, when present.
        location: Option<String> = "Location", "location", optional;
        /// Provider product that owns the resource.
        product: String = "Product", "product", value;
        /// Whether the provider marks the volume persistent.
        persistent_volume: bool = "PersistentVolume", "persistentVolume", value;
        /// Provider state for an ongoing or completed migration.
        migrate_state: String = "MigrateState", "migrateState", value;
        /// Maximum supported size in GB.
        max_size: i64 = "MaxSize", "maxSize", value;
        /// Minimum supported size in GB.
        min_size: i64 = "MinSize", "minSize", value;
        /// Zone identifier returned with the resource.
        zone_id: String = "ZoneID", "zoneId", value;
        /// Zone details associated with this resource.
        zone: Zone = "Zone", "zone", value;
    }
}

row! {
    /// Zone returned by the volume service.
    Zone {
        /// Provider-assigned UUID.
        uuid: String = "UUID", "uuid", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Provider zone classification.
        zone_type: String = "ZoneType", "zoneType", value;
        /// Whether this is the region's default zone.
        is_default: bool = "IsDefault", "isDefault", value;
        /// Resource description supplied by the provider.
        description: String = "Description", "description", value;
        /// Whether the provider marked this zone enabled.
        is_enabled: bool = "IsEnabled", "isEnabled", value;
        /// Provider OpenStack availability zone.
        openstack_zone: String = "OpenStackZone", "openstackZone", value;
        /// Network ranges associated with this zone.
        #[serde(serialize_with = "slice_or_empty")]
        ip_ranges: Option<Vec<String>> = "IPRanges", "ipRanges", optional;
        /// Count of servers associated with the zone.
        server_count: i64 = "ServerCount", "serverCount", value;
        /// Count of volumes associated with the zone.
        volume_count: i64 = "VolumeCount", "volumeCount", value;
    }
}

row! {
    /// Volume type returned by the volume service.
    VolumeType {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Identifier of the volume type associated with the resource.
        volume_type_id: String = "VolumeTypeID", "volumeTypeId", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Provisioned I/O operations per second.
        iops: i64 = "IOPS", "iops", value;
        /// Maximum supported size in GB.
        max_size: i64 = "MaxSize", "maxSize", value;
        /// Minimum supported size in GB.
        min_size: i64 = "MinSize", "minSize", value;
        /// Provisioned throughput in the provider's units.
        throughput: i64 = "Throughput", "throughPut", value;
        /// Zone identifier returned with the resource.
        zone_id: String = "ZoneID", "zoneId", value;
        /// Identifier of the volume-type zone.
        volume_type_zone_id: String = "VolumeTypeZoneID", "volumeTypeZoneId", value;
    }
}

row! {
    /// Volume type zone returned by the volume service.
    VolumeTypeZone {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Resource description supplied by the provider.
        description: String = "Description", "description", value;
        /// Zone details associated with this resource.
        zone: Zone = "Zone", "zone", value;
    }
}

row! {
    /// Encryption type returned by the volume service.
    EncryptionType {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Encryption value reported by the catalog.
        value: String = "Value", "value", value;
        /// Display label for this encryption method.
        display_key: String = "DisplayKey", "displayKey", value;
        /// Encryption key accepted in quote and create requests.
        key: String = "Key", "key", value;
        /// Resource description supplied by the provider.
        description: String = "Description", "description", value;
    }
}

row! {
    /// Snapshot returned by the volume service.
    Snapshot {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Provider-assigned UUID.
        uuid: String = "UUID", "uuid", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Resource description supplied by the provider.
        description: String = "Description", "description", value;
        /// Creation time in the provider's format.
        created_at: String = "CreatedAt", "createdAt", value;
        /// Last update time in the provider's format.
        updated_at: String = "UpdatedAt", "updatedAt", value;
        /// Deletion time in the provider's format.
        deleted_at: String = "DeletedAt", "deletedAt", value;
        /// Identifier of the source volume.
        volume_id: String = "VolumeID", "volumeId", value;
        /// Identifier of the snapshot's backing volume.
        snapshot_volume_id: String = "SnapshotVolumeID", "snapshotVolumeId", value;
        /// Identifier of the volume type associated with the resource.
        volume_type_id: String = "VolumeTypeID", "volumeTypeId", value;
        /// Identifier of the volume-type zone.
        volume_type_zone_id: String = "VolumeTypeZoneID", "volumeTypeZoneId", value;
        /// Owning project identifier.
        project_id: String = "ProjectID", "projectId", value;
        /// Provider identifier of the user who created the snapshot.
        user_id: Value = "UserID", "userId", value;
        /// Provisioned size reported by the provider.
        size: i64 = "Size", "size", value;
        /// Source volume size reported by the provider.
        volume_size: i64 = "VolumeSize", "volumeSize", value;
        /// Current provider state used for settlement.
        status: String = "Status", "status", value;
        /// Snapshot type returned by the provider.
        snapshot_type: String = "Type", "type", value;
        /// Provider product that owns the resource.
        product: String = "Product", "product", value;
        /// Identifier of the backend that owns the snapshot.
        backend_id: String = "BackendID", "backendId", value;
        /// Pool assigned by the snapshot backend.
        backend_pool: String = "BackendPool", "backendPool", value;
        /// Storage prefix reported by the snapshot backend.
        backend_prefix: String = "BackendPrefix", "backendPrefix", value;
        /// Snapshot state as reported by its backend.
        backend_status: String = "BackendStatus", "backendStatus", value;
        /// Provider UUID of the snapshot backend.
        backend_uuid: String = "BackendUUID", "backendUuid", value;
        /// Attachment position in the server's boot order.
        boot_index: i64 = "BootIndex", "bootIndex", value;
        /// Whether this volume can boot a server.
        bootable: bool = "Bootable", "bootable", value;
        /// Encryption mode returned with the resource.
        encryption_type: Value = "EncryptionType", "encryptionType", value;
        /// Whether the provider marks the snapshot permanent.
        is_permanently: bool = "IsPermanently", "isPermanently", value;
        /// Whether more than one server can attach to the volume.
        multi_attach: bool = "MultiAttach", "multiAttach", value;
        /// Identifier of the snapshot's parent resource.
        parent_id: String = "ParentID", "parentId", value;
        /// Type of the snapshot's parent resource.
        parent_type: String = "ParentType", "parentType", value;
        /// Provider policy details associated with this snapshot.
        policy_snapshot: Value = "PolicySnapshot", "policySnapshot", value;
        /// Number of days the provider retains the snapshot.
        retained_days: i64 = "RetainedDays", "retainedDays", value;
        /// Schedule classification returned by the provider.
        schedule_type: String = "ScheduleType", "scheduleType", value;
        /// Provider-specific snapshot configuration.
        snapshot_config: Value = "SnapshotConfig", "snapshotConfig", value;
        /// Snapshot time in the provider's format.
        snapshot_time: Value = "SnapshotTime", "snapshotTime", value;
        /// Provider-specific snapshot details.
        volume_snapshot: Value = "VolumeSnapshot", "volumeSnapshot", value;
    }
}

row! {
    /// Snapshot backend returned by the volume service.
    SnapshotBackend {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
    }
}

row! {
    /// Snapshot policy returned by the volume service.
    SnapshotPolicy {
        /// Identifier returned in the `id` field.
        id: String = "ID", "id", value;
        /// Display name returned by the provider.
        name: String = "Name", "name", value;
        /// Provider classification for this snapshot policy.
        policy_type: String = "PolicyType", "policyType", value;
        /// Schedule settings for this snapshot policy.
        config: SnapshotPolicyConfig = "Config", "config", value;
        /// Creation time in the provider's format.
        created_at: String = "CreatedAt", "createdAt", value;
        /// Last update time in the provider's format.
        updated_at: String = "UpdatedAt", "updatedAt", value;
        /// Count of servers associated with this policy.
        snapshot_server_count: i64 = "SnapshotServerCount", "snapshotServerCount", value;
        /// Count of volumes associated with this policy.
        snapshot_volume_count: i64 = "SnapshotVolumeCount", "snapshotVolumeCount", value;
    }
}

row! {
    /// Snapshot policy config returned by the volume service.
    SnapshotPolicyConfig {
        /// Hour of the configured snapshot schedule.
        hour: i64 = "Hour", "hour", integral;
        /// Minute of the configured snapshot schedule.
        minute: i64 = "Minute", "minute", integral;
        /// Time zone used to evaluate the configured schedule.
        time_zone: String = "TimeZone", "timeZone", value;
        /// Whether hourly snapshots are enabled.
        hourly_enabled: bool = "HourlyEnabled", "hourlyEnabled", value;
        /// Hourly cadence settings when specified.
        hourly_config: Option<SnapshotPolicyHourlyConfig> = "HourlyConfig", "hourlyConfig", optional;
        /// Whether daily snapshots are enabled.
        daily_enabled: bool = "DailyEnabled", "dailyEnabled", value;
        /// Daily retention settings when specified.
        daily_config: Option<SnapshotPolicyDailyConfig> = "DailyConfig", "dailyConfig", optional;
        /// Whether weekly snapshots are enabled.
        weekly_enabled: bool = "WeeklyEnabled", "weeklyEnabled", value;
        /// Whether monthly snapshots are enabled.
        monthly_enabled: bool = "MonthlyEnabled", "monthlyEnabled", value;
        /// Whether this policy protects servers.
        is_protected_server: bool = "IsProtectedServer", "isProtectedServer", value;
        /// Recipients for snapshot status notifications.
        #[serde(serialize_with = "slice_or_empty")]
        status_send_email: Option<Vec<String>> = "StatusSendEmail", "statusSendEmail", optional;
    }
}

row! {
    /// Snapshot policy hourly config returned by the volume service.
    SnapshotPolicyHourlyConfig {
        /// Configured hourly snapshot interval.
        interval: Option<i64> = "Interval", "interval", optional_integral;
        /// Retention setting reported by the provider.
        retention: Option<i64> = "Retention", "retention", optional_integral;
    }
}

row! {
    /// Snapshot policy daily config returned by the volume service.
    SnapshotPolicyDailyConfig {
        /// Retention setting reported by the provider.
        retention: Option<i64> = "Retention", "retention", optional_integral;
    }
}
