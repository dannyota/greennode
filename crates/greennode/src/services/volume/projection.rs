use greennode_sdk::{List, PagedList, volume as sdk};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
};

use crate::cli::render::DynamicMap;

struct Dynamic<'a>(&'a serde_json::Value);

impl Serialize for Dynamic<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            serde_json::Value::Object(items) => {
                DynamicMap(DynamicObject(items)).serialize(serializer)
            }
            serde_json::Value::Number(number) => serializer.serialize_f64(
                number
                    .as_f64()
                    .ok_or_else(|| serde::ser::Error::custom("invalid volume number"))?,
            ),
            serde_json::Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&Self(item))?;
                }
                seq.end()
            }
            value => value.serialize(serializer),
        }
    }
}

struct DynamicObject<'a>(&'a serde_json::Map<String, serde_json::Value>);

impl Serialize for DynamicObject<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0 {
            map.serialize_entry(key, &Dynamic(value))?;
        }
        map.end()
    }
}

pub(super) struct VolumeView(pub(super) sdk::Volume);

impl Serialize for VolumeView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("Volume", 31)?;
        row.serialize_field("UUID", &self.0.uuid)?;
        row.serialize_field("ID", &self.0.id)?;
        row.serialize_field("ProjectID", &self.0.project_id)?;
        row.serialize_field("Name", &self.0.name)?;
        row.serialize_field("Description", &self.0.description)?;
        row.serialize_field("Size", &self.0.size)?;
        row.serialize_field("Status", &self.0.status)?;
        row.serialize_field("StatusMessage", &self.0.status_message)?;
        row.serialize_field("VolumeTypeID", &self.0.volume_type_id)?;
        row.serialize_field("VolumeType", &Dynamic(&self.0.volume_type))?;
        row.serialize_field("VolumeTypeZoneName", &self.0.volume_type_zone_name)?;
        row.serialize_field("IOPS", &self.0.iops)?;
        row.serialize_field("Throughput", &Dynamic(&self.0.throughput))?;
        row.serialize_field("ServerID", &self.0.server_id)?;
        row.serialize_field(
            "ServerNameList",
            &self.0.server_name_list.as_deref().unwrap_or_default(),
        )?;
        row.serialize_field("CreatedAt", &self.0.created_at)?;
        row.serialize_field("UpdatedAt", &self.0.updated_at)?;
        row.serialize_field("Bootable", &self.0.bootable)?;
        row.serialize_field("EncryptionType", &self.0.encryption_type)?;
        row.serialize_field("EncryptionKeyID", &self.0.encryption_key_id)?;
        row.serialize_field("BootIndex", &self.0.boot_index)?;
        row.serialize_field("MultiAttach", &self.0.multi_attach)?;
        row.serialize_field(
            "ServerIDList",
            &self.0.server_id_list.as_deref().unwrap_or_default(),
        )?;
        row.serialize_field("Location", &self.0.location)?;
        row.serialize_field("Product", &self.0.product)?;
        row.serialize_field("PersistentVolume", &self.0.persistent_volume)?;
        row.serialize_field("MigrateState", &self.0.migrate_state)?;
        row.serialize_field("MaxSize", &self.0.max_size)?;
        row.serialize_field("MinSize", &self.0.min_size)?;
        row.serialize_field("ZoneID", &self.0.zone_id)?;
        row.serialize_field("Zone", &self.0.zone)?;
        row.end()
    }
}

pub(super) struct SnapshotView(sdk::Snapshot);

impl Serialize for SnapshotView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("Snapshot", 36)?;
        row.serialize_field("ID", &self.0.id)?;
        row.serialize_field("UUID", &self.0.uuid)?;
        row.serialize_field("Name", &self.0.name)?;
        row.serialize_field("Description", &self.0.description)?;
        row.serialize_field("CreatedAt", &self.0.created_at)?;
        row.serialize_field("UpdatedAt", &self.0.updated_at)?;
        row.serialize_field("DeletedAt", &self.0.deleted_at)?;
        row.serialize_field("VolumeID", &self.0.volume_id)?;
        row.serialize_field("SnapshotVolumeID", &self.0.snapshot_volume_id)?;
        row.serialize_field("VolumeTypeID", &self.0.volume_type_id)?;
        row.serialize_field("VolumeTypeZoneID", &self.0.volume_type_zone_id)?;
        row.serialize_field("ProjectID", &self.0.project_id)?;
        row.serialize_field("UserID", &Dynamic(&self.0.user_id))?;
        row.serialize_field("Size", &self.0.size)?;
        row.serialize_field("VolumeSize", &self.0.volume_size)?;
        row.serialize_field("Status", &self.0.status)?;
        row.serialize_field("Type", &self.0.snapshot_type)?;
        row.serialize_field("Product", &self.0.product)?;
        row.serialize_field("BackendID", &self.0.backend_id)?;
        row.serialize_field("BackendPool", &self.0.backend_pool)?;
        row.serialize_field("BackendPrefix", &self.0.backend_prefix)?;
        row.serialize_field("BackendStatus", &self.0.backend_status)?;
        row.serialize_field("BackendUUID", &self.0.backend_uuid)?;
        row.serialize_field("BootIndex", &self.0.boot_index)?;
        row.serialize_field("Bootable", &self.0.bootable)?;
        row.serialize_field("EncryptionType", &Dynamic(&self.0.encryption_type))?;
        row.serialize_field("IsPermanently", &self.0.is_permanently)?;
        row.serialize_field("MultiAttach", &self.0.multi_attach)?;
        row.serialize_field("ParentID", &self.0.parent_id)?;
        row.serialize_field("ParentType", &self.0.parent_type)?;
        row.serialize_field("PolicySnapshot", &Dynamic(&self.0.policy_snapshot))?;
        row.serialize_field("RetainedDays", &self.0.retained_days)?;
        row.serialize_field("ScheduleType", &self.0.schedule_type)?;
        row.serialize_field("SnapshotConfig", &Dynamic(&self.0.snapshot_config))?;
        row.serialize_field("SnapshotTime", &Dynamic(&self.0.snapshot_time))?;
        row.serialize_field("VolumeSnapshot", &Dynamic(&self.0.volume_snapshot))?;
        row.end()
    }
}

#[derive(Serialize)]
pub(super) struct VolumeOutput {
    #[serde(rename = "Volume")]
    volume: VolumeView,
}

pub(super) fn volume(output: sdk::GetVolumeOutput) -> VolumeOutput {
    VolumeOutput {
        volume: VolumeView(output.volume),
    }
}

pub(super) fn volumes(output: sdk::ListVolumesOutput) -> PagedList<VolumeView> {
    page(output, VolumeView)
}

pub(super) fn by_server(output: sdk::ListVolumesByServerOutput) -> List<VolumeView> {
    List {
        items: output.items.into_iter().map(VolumeView).collect(),
    }
}

pub(super) fn snapshots(output: sdk::ListSnapshotsOutput) -> PagedList<SnapshotView> {
    page(output, SnapshotView)
}

pub(super) fn all_snapshots(output: sdk::ListAllSnapshotsOutput) -> List<SnapshotView> {
    List {
        items: output.items.into_iter().map(SnapshotView).collect(),
    }
}

fn page<T, U>(output: PagedList<T>, row: impl Fn(T) -> U) -> PagedList<U> {
    PagedList {
        items: output.items.into_iter().map(row).collect(),
        page: output.page,
        page_size: output.page_size,
        total_page: output.total_page,
        total_item: output.total_item,
    }
}
