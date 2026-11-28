use super::{
    Catalog, REGIONS, array, exact, fields::zero_rows, unique, unpaged, value, wire_field,
};
use crate::go_text::go_upper;
use crate::quote::request::{Product, Request, api, invalid};
use crate::wire::product::options::PathIds;
use crate::{Error, ErrorClass};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageRegion {
    pub(crate) region_id: String,
    pub(crate) region_name: String,
}

pub(crate) fn storage_region_name(configured: &str, name: &str) -> Option<String> {
    let name = if name.is_empty() {
        match configured {
            "hcm-3" => "HCM04",
            "han-1" => "HAN02",
            _ => return None,
        }
    } else {
        name
    };
    Some(go_upper(name))
}

pub(crate) fn storage_region<'a>(
    regions: &'a [StorageRegion],
    name: &str,
) -> Option<&'a StorageRegion> {
    regions
        .iter()
        .find(|row| go_upper(&row.region_name) == name || row.region_id.eq_ignore_ascii_case(name))
}

pub(crate) fn storage_data<'a>(
    op: &'static str,
    raw: &'a Value,
    list: bool,
) -> Result<&'a Value, Box<Error>> {
    let obj = raw.as_object().ok_or_else(|| invalid(op, 200))?;
    if obj
        .get("errorMsg")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(invalid(op, 200));
    }
    let next = match obj.get("isNext") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(next)) => *next,
        _ => return Err(invalid(op, 200)),
    };
    let success = obj.get("success").and_then(Value::as_bool);
    if list || success == Some(true) {
        unpaged(op, obj, true)?;
        if next {
            return Err(invalid(op, 200));
        }
    }
    match success {
        Some(true) => {}
        Some(false) => {
            let code = obj
                .get("code")
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
            let mut error = api(
                op,
                Some(200),
                &code.map_or_else(String::new, |code| code.to_string()),
                "storage request refused; body withheld",
                false,
            );
            if let Some(class) = match code {
                Some(112) => Some(ErrorClass::InvalidInput),
                Some(401) => Some(ErrorClass::Auth),
                Some(403) => Some(ErrorClass::PermissionDenied),
                Some(404) => Some(ErrorClass::NotFound),
                Some(429) => Some(ErrorClass::RateLimited),
                _ => None,
            } {
                error = error.with_cause(Error::new(class));
            }
            return Err(Box::new(error));
        }
        None => return Err(invalid(op, 200)),
    }
    (if list {
        obj.get("datas").or_else(|| obj.get("data"))
    } else {
        obj.get("data")
    })
    .ok_or_else(|| invalid(op, 200))
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct StorageResource {
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) id: i64,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) price_key: String,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) purchase_type_id: i64,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) project_type_id: i64,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) period: Option<i64>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageType {
    pub(crate) id: i64,
    pub(crate) name: String,
    #[serde(default, deserialize_with = "wire_field")]
    #[allow(dead_code, reason = "Shared storage catalog field")]
    pub(crate) title: String,
    pub(crate) status: i64,
    #[serde(deserialize_with = "wire_field")]
    pub(crate) allow_period: Vec<i64>,
    #[serde(deserialize_with = "zero_rows")]
    pub(crate) billable_resources: Vec<StorageResource>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
pub(crate) struct StoragePurchase {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) status: i64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct StorageCatalog {
    pub(crate) types: Vec<StorageType>,
    pub(crate) purchases: Vec<StoragePurchase>,
    pub(crate) min_quota: i64,
    pub(crate) max_quota: i64,
    pub(crate) max_projects: i64,
    pub(crate) iam_checkout: bool,
}

impl StorageCatalog {
    pub(crate) fn resolve(&self, name: &str, quota: i64) -> Option<(i64, i64)> {
        if !(self.min_quota..=self.max_quota).contains(&quota) {
            return None;
        }
        let kind = self
            .types
            .iter()
            .find(|kind| kind.name == name && kind.status == 1 && kind.allow_period.contains(&1))?;
        let purchase = monthly(&self.purchases).ok().flatten()?;
        storage_price_key(kind, purchase.id)?;
        Some((purchase.id, kind.id))
    }
}

pub(crate) fn storage_price_key(kind: &StorageType, purchase: i64) -> Option<&str> {
    let mut resources = kind
        .billable_resources
        .iter()
        .filter(|row| row.purchase_type_id == purchase);
    let row = resources.next()?;
    (resources.next().is_none()
        && row.project_type_id == kind.id
        && row.period.is_none_or(|period| period == 1))
    .then_some(row.price_key.as_str())
}

fn monthly(rows: &[StoragePurchase]) -> Result<Option<&StoragePurchase>, ()> {
    let mut offerings = rows
        .iter()
        .filter(|row| row.status == 1 && row.name == "Normal" && row.title == "Pay monthly");
    let first = offerings.next();
    if offerings.next().is_some() {
        Err(())
    } else {
        Ok(first)
    }
}

impl Catalog<'_> {
    pub(crate) async fn storage_regions(&mut self) -> Result<Vec<StorageRegion>, Box<Error>> {
        let raw = self
            .storage_read(REGIONS, "internal/v1/regions", None, &[])
            .await?;
        let rows: Vec<StorageRegion> = array(REGIONS, &raw, None, &["regionId", "regionName"])?;
        unique(
            REGIONS,
            rows.iter().map(|row| row.region_id.as_str()),
            PathIds::Strict,
        )?;
        let mut aliases = HashMap::new();
        for row in &rows {
            if row.region_name.is_empty()
                || aliases
                    .insert(go_upper(&row.region_name), &row.region_id)
                    .is_some()
            {
                return Err(invalid(REGIONS, 200));
            }
        }
        for row in &rows {
            if aliases
                .insert(row.region_id.to_ascii_uppercase(), &row.region_id)
                .is_some_and(|id| id != &row.region_id)
            {
                return Err(invalid(REGIONS, 200));
            }
        }
        self.rows(rows.len())?;
        Ok(rows)
    }

    async fn storage_read(
        &mut self,
        op: &'static str,
        template: &'static str,
        id: Option<&str>,
        query: &[(&str, &str)],
    ) -> Result<Value, Box<Error>> {
        let raw = self
            .exchange(Request {
                operation: op,
                template,
                product: Product::Storage(id),
                path: &[],
                query,
                body: None,
            })
            .await?;
        let data = value(op, &raw)?;
        let rows = storage_data(op, &data, true)?;
        if !rows.is_array() {
            return Err(invalid(op, 200));
        }
        Ok(rows.clone())
    }

    pub(crate) async fn storage_catalog(
        &mut self,
        op: &'static str,
        id: &str,
    ) -> Result<StorageCatalog, Box<Error>> {
        let kinds = self
            .storage_read(op, "internal/v1/billing/project_types", Some(id), &[])
            .await?;
        let types: Vec<StorageType> = array(
            op,
            &kinds,
            None,
            &[
                "id",
                "name",
                "title",
                "status",
                "allowPeriod",
                "billableResources",
            ],
        )?;
        for row in kinds.as_array().into_iter().flatten() {
            if ["allowPeriod", "billableResources"]
                .into_iter()
                .any(|key| row.get(key).is_none_or(Value::is_null))
            {
                return Err(invalid(op, 200));
            }
            for item in row
                .get("billableResources")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if item.is_null() {
                    continue;
                }
                exact(
                    op,
                    item.as_object().ok_or_else(|| invalid(op, 200))?,
                    &[
                        "id",
                        "name",
                        "priceKey",
                        "purchaseTypeId",
                        "projectTypeId",
                        "period",
                    ],
                )?;
            }
        }
        self.rows(types.len())?;
        let raw = self
            .storage_read(op, "internal/v1/billing/purchase_types", Some(id), &[])
            .await?;
        let purchases: Vec<StoragePurchase> =
            array(op, &raw, None, &["id", "name", "title", "status"])?;
        self.rows(purchases.len())?;
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        if types.iter().any(|row| {
            row.id <= 0 || row.name.is_empty() || !ids.insert(row.id) || !names.insert(&row.name)
        }) {
            return Err(invalid(op, 200));
        }
        ids.clear();
        if purchases
            .iter()
            .any(|row| row.id <= 0 || row.name.is_empty() || !ids.insert(row.id))
        {
            return Err(invalid(op, 200));
        }
        let offering = monthly(&purchases).map_err(|()| invalid(op, 200))?;
        for row in &types {
            self.rows(row.billable_resources.len())?;
            if let Some(offer) = offering
                && row.status == 1
                && row.allow_period.contains(&1)
                && storage_price_key(row, offer.id).is_none()
            {
                return Err(invalid(op, 200));
            }
        }
        let min_quota = self
            .configuration_integer(op, id, "vos_billing_normal_min_quota")
            .await?;
        let max_quota = self
            .configuration_integer(op, id, "vos_billing_normal_max_quota")
            .await?;
        let max_projects = self
            .configuration_integer(op, id, "max_project_per_user_per_region")
            .await?;
        if min_quota > max_quota {
            return Err(invalid(op, 200));
        }
        let iam_checkout = match self
            .configuration(op, id, "enable_iam_checkout")
            .await?
            .as_str()
        {
            "true" => true,
            "false" => false,
            _ => return Err(invalid(op, 200)),
        };
        Ok(StorageCatalog {
            types,
            purchases,
            min_quota,
            max_quota,
            max_projects,
            iam_checkout,
        })
    }

    async fn configuration(
        &mut self,
        op: &'static str,
        id: &str,
        key: &str,
    ) -> Result<String, Box<Error>> {
        let raw = self
            .storage_read(
                op,
                "billing-api/v1/configurations",
                Some(id),
                &[("keys", key), ("region_id", id)],
            )
            .await?;
        let [row] = raw.as_array().map(Vec::as_slice).unwrap_or_default() else {
            return Err(invalid(op, 200));
        };
        exact(
            op,
            row.as_object().ok_or_else(|| invalid(op, 200))?,
            &["key", "value"],
        )?;
        let value = row
            .get("value")
            .and_then(Value::as_str)
            .filter(|_| row.get("key").and_then(Value::as_str) == Some(key));
        let value = value.ok_or_else(|| invalid(op, 200))?.to_owned();
        self.rows(1)?;
        Ok(value)
    }

    async fn configuration_integer(
        &mut self,
        op: &'static str,
        id: &str,
        key: &str,
    ) -> Result<i64, Box<Error>> {
        let value = self.configuration(op, id, key).await?;
        value
            .parse::<i64>()
            .ok()
            .filter(|number| *number > 0 && value.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| invalid(op, 200))
    }
}
