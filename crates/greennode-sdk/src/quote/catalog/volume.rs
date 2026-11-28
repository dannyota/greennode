use super::{Catalog, ENCRYPTIONS, PARENTS, TYPES, array, exact, unique, wire_field};
use crate::{Error, quote::request::invalid, wire::product::options::PathIds};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
#[allow(dead_code, reason = "Shared volume-zone fields for service ports")]
pub(crate) struct ParentZone {
    #[serde(deserialize_with = "wire_field")]
    pub(crate) uuid: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) zone_type: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) is_default: bool,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) description: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) is_enabled: bool,
    #[serde(rename = "openstackZone")]
    #[serde(deserialize_with = "wire_field")]
    pub(crate) openstack_zone: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) ip_ranges: Option<Vec<String>>,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) server_count: i64,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) volume_count: i64,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(default)]
#[derive(Default)]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct VolumeParent {
    #[serde(deserialize_with = "wire_field")]
    pub(crate) id: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) description: String,
    pub(crate) zone: Option<ParentZone>,
}

#[derive(Clone, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct VolumeType {
    #[serde(deserialize_with = "wire_field")]
    pub(crate) id: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) volume_type_id: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) iops: i64,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) max_size: i64,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) min_size: i64,
    #[serde(rename = "throughPut")]
    #[serde(deserialize_with = "wire_field")]
    pub(crate) throughput: i64,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) zone_id: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) volume_type_zone_id: String,
}

impl VolumeType {
    pub(crate) fn quote_id(&self) -> &str {
        if self.volume_type_id.is_empty() {
            &self.id
        } else {
            &self.volume_type_id
        }
    }
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct Encryption {
    #[serde(deserialize_with = "wire_field")]
    pub(crate) id: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) value: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) display_key: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) key: String,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) description: String,
}

impl Catalog<'_> {
    pub(crate) async fn volume_parents(
        &mut self,
        project: &str,
        validation_op: &'static str,
    ) -> Result<Vec<VolumeParent>, Box<Error>> {
        let raw = self
            .regional(
                PARENTS,
                validation_op,
                "v1/{}/volume_type_zones",
                &[project],
                &[],
            )
            .await?;
        let rows: Vec<VolumeParent> = array(
            validation_op,
            &raw,
            Some("volumeTypeZones"),
            &["id", "name", "description", "zone"],
        )?;
        unique(
            validation_op,
            rows.iter().map(|row| row.id.as_str()),
            PathIds::Strict,
        )?;
        for row in raw
            .get("volumeTypeZones")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(zone) = row.get("zone").and_then(Value::as_object) {
                exact(
                    validation_op,
                    zone,
                    &[
                        "uuid",
                        "name",
                        "zoneType",
                        "isDefault",
                        "description",
                        "isEnabled",
                        "openstackZone",
                        "ipRanges",
                        "serverCount",
                        "volumeCount",
                    ],
                )?;
            }
        }
        self.rows(rows.len())?;
        Ok(rows)
    }

    pub(crate) async fn volume_types(
        &mut self,
        project: &str,
        parent: &str,
        details: bool,
        validation_op: &'static str,
    ) -> Result<Vec<VolumeType>, Box<Error>> {
        let raw = self
            .regional(
                TYPES,
                validation_op,
                "v1/{}/{}/volume_types",
                &[project, parent],
                &[],
            )
            .await?;
        let rows: Vec<VolumeType> = array(
            validation_op,
            &raw,
            Some("volumeTypes"),
            &[
                "id",
                "volumeTypeId",
                "name",
                "iops",
                "minSize",
                "maxSize",
                "throughPut",
                "zoneId",
                "volumeTypeZoneId",
            ],
        )?;
        unique(
            validation_op,
            rows.iter().map(VolumeType::quote_id),
            PathIds::Strict,
        )?;
        if rows.iter().any(|row| {
            row.min_size <= 0
                || row.max_size < row.min_size
                || (!row.volume_type_zone_id.is_empty() && row.volume_type_zone_id != parent)
                || (details && (row.name.is_empty() || row.iops < 0))
        }) {
            return Err(invalid(validation_op, 200));
        }
        self.rows(rows.len())?;
        Ok(rows)
    }

    pub(crate) async fn volume_catalog(
        &mut self,
        project: &str,
    ) -> Result<Vec<VolumeType>, Box<Error>> {
        let mut indexes = HashMap::new();
        let mut rows = Vec::new();
        for parent in self.volume_parents(project, PARENTS).await? {
            for row in self.volume_types(project, &parent.id, false, TYPES).await? {
                if let Some(&index) = indexes.get(row.quote_id()) {
                    if rows.get(index) != Some(&row) {
                        return Err(invalid(TYPES, 200));
                    }
                } else {
                    indexes.insert(row.quote_id().to_owned(), rows.len());
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }

    pub(crate) async fn encryptions(
        &mut self,
        project: &str,
        validation_op: &'static str,
    ) -> Result<Vec<Encryption>, Box<Error>> {
        let raw = self
            .regional(
                ENCRYPTIONS,
                validation_op,
                "v1/{}/volumes/encryption_types",
                &[project],
                &[],
            )
            .await?;
        let rows: Vec<Encryption> = array(
            validation_op,
            &raw,
            None,
            &["id", "name", "value", "displayKey", "key", "description"],
        )?;
        unique(
            validation_op,
            rows.iter().map(|row| row.key.as_str()),
            PathIds::WithUnderscore,
        )?;
        if rows
            .iter()
            .any(|row| matches!(row.key.as_str(), "none" | "all"))
        {
            return Err(invalid(validation_op, 200));
        }
        self.rows(rows.len())?;
        Ok(rows)
    }
}
