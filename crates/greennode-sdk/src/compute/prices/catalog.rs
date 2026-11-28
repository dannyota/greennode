use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::model::pricing::DiscoveryPrice;
use crate::model::wire::Field;
use crate::quote::catalog::{self, Catalog, Zone, array, array_rows, exact, wire_field};
use crate::quote::request::{Product, Request, invalid};
use crate::wire::product::options::PathIds;
use crate::{Error, ErrorClass};

use super::super::{Flavor, FlavorZone};
use super::run::Run;
use super::{FlavorPrice, ListFlavorPricesInput};

const PLATFORMS: &str = "compute.ListFlavorPlatforms";
const FAMILIES: &str = "compute.ListFlavorFamilies";
const FAMILY_FLAVORS: &str = "compute.ListFamilyFlavors";
const GROUPS: &str = "compute.ListFlavorZones";
const CUSTOM: &str = "compute.ListCustomFlavors";
const LEGACY: &str = "compute.ListFlavors";

#[derive(Deserialize)]
pub(super) struct Platform {
    #[serde(default, deserialize_with = "wire_field")]
    key: String,
    #[serde(default, rename = "value", deserialize_with = "wire_field")]
    _value: String,
    #[serde(default, rename = "description", deserialize_with = "wire_field")]
    _description: String,
}

#[derive(Deserialize)]
struct Family {
    #[serde(default, deserialize_with = "wire_field")]
    key: String,
    #[serde(default, deserialize_with = "wire_field")]
    value: String,
    #[serde(default, rename = "description", deserialize_with = "wire_field")]
    _description: String,
    condition: Condition,
}

#[derive(Deserialize)]
struct Condition {
    #[serde(deserialize_with = "wire_field")]
    codes: Option<Vec<String>>,
}

async fn get(
    catalog: &mut Catalog<'_>,
    operation: &'static str,
    template: &'static str,
    path: &[&str],
    query: &[(&str, &str)],
) -> Result<Value, Box<Error>> {
    let raw = catalog
        .exchange(Request {
            operation,
            template,
            product: Product::Regional,
            path,
            query,
            body: None,
        })
        .await?;
    catalog::value(operation, &raw)
}

fn unique_ids<'a>(ids: impl IntoIterator<Item = &'a str>, rule: PathIds) -> bool {
    let mut seen = HashSet::new();
    ids.into_iter().all(|id| rule.allows(id) && seen.insert(id))
}

impl Run<'_> {
    pub(super) async fn platforms(&mut self, project: &str) -> Result<Vec<Platform>, Box<Error>> {
        let raw = get(
            &mut self.catalog,
            PLATFORMS,
            "v1/{}/flavor_zones/codes",
            &[project],
            &[],
        )
        .await?;
        let rows: Vec<Platform> = array(PLATFORMS, &raw, None, &["key", "value", "description"])?;
        if !unique_ids(
            rows.iter().map(|platform| platform.key.as_str()),
            PathIds::Strict,
        ) {
            return Err(invalid(PLATFORMS, 200));
        }
        self.catalog.rows(rows.len())?;
        Ok(rows)
    }

    pub(super) async fn console(
        &mut self,
        project: &str,
        zone: &Zone,
        platforms: &[Platform],
    ) -> Result<(), Box<Error>> {
        let query = [("zoneId", zone.id.as_str())];
        let raw = get(
            &mut self.catalog,
            FAMILIES,
            "v1/{}/flavor_zones/families",
            &[project],
            &query,
        )
        .await?;
        let families: Vec<Family> = array(
            FAMILIES,
            &raw,
            None,
            &["key", "value", "description", "condition", "types"],
        )?;
        for condition in array_rows(FAMILIES, &raw, None)?
            .iter()
            .filter_map(|row| row.get("condition").and_then(Value::as_object))
        {
            exact(FAMILIES, condition, &["codes"])?;
        }
        if !unique_ids(
            families.iter().map(|family| family.key.as_str()),
            PathIds::Strict,
        ) || families.iter().any(|family| {
            family.condition.codes.as_ref().is_none_or(|codes| {
                !unique_ids(codes.iter().map(String::as_str), PathIds::Strict)
                    || codes
                        .iter()
                        .any(|code| !platforms.iter().any(|platform| platform.key == *code))
            })
        }) {
            return Err(invalid(FAMILIES, 200));
        }
        self.catalog.rows(families.len())?;
        for family in &families {
            if family.key == "custom" || !matches_filter(&self.input.family, &family.key) {
                continue;
            }
            for platform in family.condition.codes.iter().flatten() {
                if !matches_filter(&self.input.platform, platform) {
                    continue;
                }
                let scope = format!("{}/{}/{}", zone.id, family.key, platform);
                if family.key == "gpu" {
                    self.note("excluded-family", &scope, "GPU family flavors are excluded; GPU reference selection and pricing remain unverified.");
                    continue;
                }
                let result = get(
                    &mut self.catalog,
                    FAMILY_FLAVORS,
                    "v1/{}/flavors/families/{}/platforms/{}",
                    &[project, &family.key, platform],
                    &query,
                )
                .await
                .and_then(|raw| {
                    self.flavors(
                        FAMILY_FLAVORS,
                        &raw,
                        None,
                        zone,
                        (&family.key, &family.value, platform),
                    )
                });
                if let Err(error) = result {
                    self.record(FAMILY_FLAVORS, &scope, error)?;
                    if self.stopped {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) async fn non_public(
        &mut self,
        project: &str,
        zones: &[Zone],
    ) -> Result<(), Box<Error>> {
        let raw = get(
            &mut self.catalog,
            GROUPS,
            "v1/{}/flavor_zones/product",
            &[project],
            &[],
        )
        .await?;
        let groups: Vec<FlavorZone> = array(GROUPS, &raw, Some("flavorZones"), &[])?;
        if !unique_ids(
            groups.iter().map(|group| group.id.as_str()),
            PathIds::Strict,
        ) || groups
            .iter()
            .any(|group| !PathIds::Strict.allows(&group.zone_id))
        {
            return Err(invalid(GROUPS, 200));
        }
        self.catalog.rows(groups.len())?;
        self.output.metadata.excluded_groups = i64::try_from(groups.len()).ok();
        self.note("non-public", "compute", "Product group count describes the excluded console-family scope. Populated custom and GPU schemas remain unverified.");
        for zone in zones {
            let query = [("zoneId", zone.id.as_str())];
            for template in ["v1/{}/flavor_zones/customs", "v1/{}/flavors/customs"] {
                let result = get(&mut self.catalog, CUSTOM, template, &[project], &query)
                    .await
                    .and_then(|raw| {
                        let rows = array_rows(CUSTOM, &raw, None)?;
                        if !rows.is_empty() {
                            return Err(invalid(CUSTOM, 200));
                        }
                        self.catalog.rows(0)
                    });
                if let Err(error) = result {
                    self.record(CUSTOM, &zone.id, error)?;
                    if self.stopped {
                        return Ok(());
                    }
                }
            }
            for group in groups.iter().filter(|group| group.zone_id == zone.id) {
                let result = get(
                    &mut self.catalog,
                    LEGACY,
                    "v1/{}/{}/flavors",
                    &[project, &group.id],
                    &[],
                )
                .await
                .and_then(|raw| self.flavors(LEGACY, &raw, Some("flavors"), zone, ("", "", "")));
                if let Err(error) = result {
                    self.record(LEGACY, &format!("{}/{}", zone.id, group.id), error)?;
                    if self.stopped {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn flavors(
        &mut self,
        op: &'static str,
        raw: &Value,
        key: Option<&str>,
        zone: &Zone,
        scope: (&str, &str, &str),
    ) -> Result<(), Box<Error>> {
        let (family, family_name, platform) = scope;
        let entries = array_rows(op, raw, key)?;
        let mut identities = HashMap::new();
        let mut tuples = HashMap::new();
        let mut rows = Vec::new();
        for entry in entries {
            let f = decode_flavor(op, entry)?;
            if [&f.flavor_id, &f.flavor_zone_id]
                .into_iter()
                .any(|id| !PathIds::Strict.allows(id))
                || f.name.is_empty()
                || f.cpu < 0
                || f.memory < 0
                || f.gpu > 0
                || (!f.zone_id.is_empty() && f.zone_id != zone.id)
            {
                return Err(invalid(op, 200));
            }
            let identity = (zone.id.clone(), f.flavor_id.clone());
            let mut comparable = f.clone();
            comparable.flavor_zone_id.clear();
            comparable.zone_id.clear();
            if self
                .identities
                .get(&identity)
                .or_else(|| identities.get(&identity))
                .is_some_and(|old| *old != comparable)
            {
                return Err(invalid(op, 200));
            }
            identities.insert(identity, comparable);
            let tuple = format!(
                "{}/{}/{}/{}/{}",
                zone.id, f.flavor_zone_id, f.flavor_id, family, platform
            );
            if let Some(old) = self.tuples.get(&tuple).or_else(|| tuples.get(&tuple)) {
                if old != &f {
                    return Err(invalid(op, 200));
                }
                continue;
            }
            tuples.insert(tuple, f.clone());
            if !matches_row(self.input, &f, family, platform) {
                continue;
            }
            rows.push(f);
        }
        let bytes_per_row = [
            zone.id.len(),
            zone.name.len(),
            family.len(),
            family_name.len(),
            platform.len(),
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add);
        self.catalog.rows_with_bytes(
            entries.len().saturating_add(rows.len()),
            bytes_per_row.saturating_mul(rows.len()),
        )?;
        self.identities.extend(identities);
        self.tuples.extend(tuples);
        let inapplicable = self.input.zone_id.is_empty() && zone.enabled == Some(false);
        self.output.items.extend(rows.into_iter().map(|f| {
            FlavorPrice {
                flavor: f,
                zone_id: zone.id.clone(),
                zone_name: zone.name.clone(),
                zone_enabled: zone.enabled,
                family: family.into(),
                family_name: family_name.into(),
                platform: platform.into(),
                visibility: if key.is_some() {
                    "unclassified"
                } else {
                    "console-family"
                }
                .into(),
                ram_unit: "unknown".into(),
                ram_gib: None,
                price_per_gib: None,
                price_per_ram_unit: None,
                reference: None,
                price: DiscoveryPrice {
                    status: if inapplicable {
                        "inapplicable"
                    } else {
                        "unknown"
                    }
                    .into(),
                    source: "pricing.GetQuote".into(),
                    ..DiscoveryPrice::default()
                },
            }
        }));
        Ok(())
    }
}

fn decode_flavor(op: &'static str, raw: &Value) -> Result<Flavor, Box<Error>> {
    let mut fields = raw.as_object().ok_or_else(|| invalid(op, 200))?.clone();
    for name in ["cpu", "memory", "remainingVms", "isSoldOut"] {
        if fields.get(name).is_none_or(Value::is_null) {
            return Err(invalid(op, 200));
        }
    }
    let metadata = fields.remove("metaData");
    let Field(mut flavor): Field<Flavor> = catalog::record(op, &Value::Object(fields), &[])?;
    flavor.metadata = metadata
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(flavor)
}

fn matches_filter(filter: &str, value: &str) -> bool {
    filter.is_empty() || filter == value
}

fn matches_row(input: &ListFlavorPricesInput, f: &Flavor, family: &str, platform: &str) -> bool {
    matches_filter(&input.family, family)
        && matches_filter(&input.platform, platform)
        && matches_filter(&input.name, &f.name)
        && f.memory >= input.min_ram
        && (input.max_ram == 0 || f.memory <= input.max_ram)
        && (!input.available_only || (!f.is_sold_out && f.remaining_vms > 0))
}

pub(super) fn stops(error: &Error) -> bool {
    [
        ErrorClass::Auth,
        ErrorClass::RateLimited,
        ErrorClass::NoCredentials,
        ErrorClass::CredentialsFile,
        ErrorClass::Cancelled,
        ErrorClass::DeadlineExceeded,
        ErrorClass::EstimateLimit,
    ]
    .into_iter()
    .any(|class| error.has_class(class))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_throttled_catalog_stops_further_requests() {
        assert!(super::stops(&crate::Error::new(
            crate::ErrorClass::RateLimited
        )));
    }
}
