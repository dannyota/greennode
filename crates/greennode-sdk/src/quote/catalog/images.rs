use super::{Catalog, IMAGES, array, exact, unique};
use crate::{Error, quote::request::invalid, wire::product::options::PathIds};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Image {
    pub(crate) id: String,
    pub(crate) image_type: String,
    pub(crate) image_version: String,
    licence: Option<Value>,
    package_limit: Option<serde_json::Map<String, Value>>,
}

impl Image {
    pub(crate) fn unlicensed_ubuntu(&self) -> bool {
        self.image_type.eq_ignore_ascii_case("ubuntu")
            && self.licence.as_ref().and_then(Value::as_bool) == Some(false)
    }

    #[allow(dead_code, reason = "Shared OS-image minima for compute pricing")]
    pub(crate) fn minimum_cpu(&self) -> Option<i64> {
        self.package_limit.as_ref()?.get("cpu")?.as_i64()
    }

    #[allow(dead_code, reason = "Shared OS-image minima for compute pricing")]
    pub(crate) fn minimum_memory(&self) -> Option<i64> {
        self.package_limit.as_ref()?.get("memory")?.as_i64()
    }

    pub(crate) fn root_minimum(&self) -> Option<i64> {
        self.package_limit
            .as_ref()?
            .get("diskSize")?
            .as_i64()
            .filter(|size| *size > 0)
    }
}

impl Catalog<'_> {
    pub(crate) async fn images(
        &mut self,
        project: &str,
        zone: &str,
    ) -> Result<Vec<Image>, Box<Error>> {
        let raw = self
            .regional(IMAGES, "v1/{}/images/os", &[project], &[("zoneId", zone)])
            .await?;
        let rows: Vec<Image> = array(
            IMAGES,
            &raw,
            Some("images"),
            &["id", "imageType", "imageVersion", "licence", "packageLimit"],
        )?;
        unique(
            IMAGES,
            rows.iter().map(|row| row.id.as_str()),
            PathIds::Strict,
        )?;
        for row in &rows {
            if row.image_type.is_empty() || row.image_version.is_empty() {
                return Err(invalid(IMAGES, 200));
            }
            if let Some(limits) = &row.package_limit {
                exact(IMAGES, limits, &["cpu", "memory", "diskSize"])?;
            }
        }
        self.rows(rows.len())?;
        Ok(rows)
    }
}
