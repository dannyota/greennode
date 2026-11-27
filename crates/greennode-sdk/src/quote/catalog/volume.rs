use super::{Catalog, ENCRYPTIONS, PARENTS, TYPES, array, exact, unique};
use crate::{Error, quote::request::invalid, wire::product::options::PathIds};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
#[allow(dead_code, reason = "Shared volume-zone fields for service ports")]
pub(crate) struct ParentZone {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) zone_type: String,
    pub(crate) is_default: bool,
    pub(crate) description: String,
    pub(crate) is_enabled: bool,
    #[serde(rename = "openstackZone")]
    pub(crate) openstack_zone: String,
    pub(crate) ip_ranges: Option<Vec<String>>,
    pub(crate) server_count: i64,
    pub(crate) volume_count: i64,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(default)]
#[derive(Default)]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct VolumeParent {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) zone: Option<ParentZone>,
}

#[derive(Clone, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct VolumeType {
    pub(crate) id: String,
    pub(crate) volume_type_id: String,
    pub(crate) name: String,
    pub(crate) iops: i64,
    pub(crate) max_size: i64,
    pub(crate) min_size: i64,
    #[serde(rename = "throughPut")]
    pub(crate) throughput: i64,
    pub(crate) zone_id: String,
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
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) display_key: String,
    pub(crate) key: String,
    pub(crate) description: String,
}

impl Catalog<'_> {
    pub(crate) async fn volume_parents(
        &mut self,
        project: &str,
    ) -> Result<Vec<VolumeParent>, Box<Error>> {
        let raw = self
            .regional(PARENTS, "v1/{}/volume_type_zones", &[project], &[])
            .await?;
        let rows: Vec<VolumeParent> = array(
            PARENTS,
            &raw,
            Some("volumeTypeZones"),
            &["id", "name", "description", "zone"],
        )?;
        unique(
            PARENTS,
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
                    PARENTS,
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
    ) -> Result<Vec<VolumeType>, Box<Error>> {
        let raw = self
            .regional(TYPES, "v1/{}/{}/volume_types", &[project, parent], &[])
            .await?;
        let rows: Vec<VolumeType> = array(
            TYPES,
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
            TYPES,
            rows.iter().map(VolumeType::quote_id),
            PathIds::WithUnderscore,
        )?;
        if rows.iter().any(|row| {
            row.min_size <= 0
                || row.max_size < row.min_size
                || (!row.volume_type_zone_id.is_empty() && row.volume_type_zone_id != parent)
                || (details && (row.name.is_empty() || row.iops < 0))
        }) {
            return Err(invalid(TYPES, 200));
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
        for parent in self.volume_parents(project).await? {
            for row in self.volume_types(project, &parent.id, false).await? {
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
    ) -> Result<Vec<Encryption>, Box<Error>> {
        let raw = self
            .regional(
                ENCRYPTIONS,
                "v1/{}/volumes/encryption_types",
                &[project],
                &[],
            )
            .await?;
        let rows: Vec<Encryption> = array(
            ENCRYPTIONS,
            &raw,
            None,
            &["id", "name", "value", "displayKey", "key", "description"],
        )?;
        unique(
            ENCRYPTIONS,
            rows.iter().map(|row| row.key.as_str()),
            PathIds::WithUnderscore,
        )?;
        if rows
            .iter()
            .any(|row| matches!(row.key.as_str(), "none" | "all"))
        {
            return Err(invalid(ENCRYPTIONS, 200));
        }
        self.rows(rows.len())?;
        Ok(rows)
    }
}
