use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::model::pricing::DiscoveryPrice;
use crate::model::wire::Field;
use crate::quote::catalog::{Catalog, Zone};
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
    key: String,
}

#[derive(Deserialize)]
struct Family {
    key: String,
    #[serde(default)]
    value: String,
    condition: Condition,
}

#[derive(Deserialize)]
struct Condition {
    codes: Vec<String>,
}

async fn get(
    catalog: &mut Catalog<'_>,
    operation: &'static str,
    template: &'static str,
    path: &[&str],
    query: &[(&str, &str)],
) -> Result<Box<RawValue>, Box<Error>> {
    catalog
        .exchange(Request {
            operation,
            template,
            product: Product::Regional,
            path,
            query,
            body: None,
        })
        .await
}

fn array<'a, T: Deserialize<'a>>(
    op: &'static str,
    raw: &'a RawValue,
    key: Option<&str>,
) -> Result<Vec<T>, Box<Error>> {
    let body = if let Some(key) = key {
        let value = crate::quote::catalog::value(op, raw)?;
        let object = value.as_object().ok_or_else(|| invalid(op, 200))?;
        crate::quote::catalog::unpaged(op, object, false)?;
        crate::quote::catalog::exact(op, object, &[key])?;
        if object
            .get("success")
            .is_some_and(|value| value.as_bool() != Some(true))
        {
            return Err(invalid(op, 200));
        }
        let fields: HashMap<String, &RawValue> =
            serde_json::from_str(raw.get()).map_err(|_| invalid(op, 200))?;
        fields.get(key).ok_or_else(|| invalid(op, 200))?.get()
    } else {
        raw.get()
    };
    serde_json::from_str(body).map_err(|_| invalid(op, 200))
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
        let rows: Vec<Platform> = array(PLATFORMS, &raw, None)?;
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
        let families: Vec<Family> = array(FAMILIES, &raw, None)?;
        if !unique_ids(
            families.iter().map(|family| family.key.as_str()),
            PathIds::Strict,
        ) || families.iter().any(|family| {
            !unique_ids(
                family.condition.codes.iter().map(String::as_str),
                PathIds::Strict,
            ) || family
                .condition
                .codes
                .iter()
                .any(|code| !platforms.iter().any(|platform| platform.key == *code))
        }) {
            return Err(invalid(FAMILIES, 200));
        }
        self.catalog.rows(families.len())?;
        for family in &families {
            if family.key == "custom" || !matches_filter(&self.input.family, &family.key) {
                continue;
            }
            for platform in &family.condition.codes {
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
        let groups: Vec<FlavorZone> = array(GROUPS, &raw, Some("flavorZones"))?;
        if !unique_ids(
            groups.iter().map(|group| group.id.as_str()),
            PathIds::Strict,
        ) || groups
            .iter()
            .any(|group| !PathIds::WithUnderscore.allows(&group.zone_id))
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
                        let rows: Vec<&RawValue> = array(CUSTOM, &raw, None)?;
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
        raw: &RawValue,
        key: Option<&str>,
        zone: &Zone,
        scope: (&str, &str, &str),
    ) -> Result<(), Box<Error>> {
        let (family, family_name, platform) = scope;
        let entries = array::<&RawValue>(op, raw, key)?;
        self.catalog.rows(entries.len())?;
        let mut identities = HashMap::new();
        let mut tuples = HashMap::new();
        let mut rows = Vec::new();
        for entry in &entries {
            let f = decode_flavor(op, entry)?;
            if [&f.flavor_id, &f.flavor_zone_id]
                .into_iter()
                .any(|id| !PathIds::WithUnderscore.allows(id))
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
            let retained_bytes = [
                zone.id.len(),
                zone.name.len(),
                family.len(),
                family_name.len(),
                platform.len(),
            ]
            .into_iter()
            .fold(0usize, usize::saturating_add);
            self.catalog.rows_with_bytes(1, retained_bytes)?;
            let inapplicable = self.input.zone_id.is_empty() && zone.enabled == Some(false);
            rows.push(FlavorPrice {
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
            });
        }
        self.identities.extend(identities);
        self.tuples.extend(tuples);
        self.output.items.extend(rows);
        Ok(())
    }
}

fn decode_flavor(op: &'static str, raw: &RawValue) -> Result<Flavor, Box<Error>> {
    let mut fields: std::collections::BTreeMap<String, &RawValue> =
        serde_json::from_str(raw.get()).map_err(|_| invalid(op, 200))?;
    for name in ["cpu", "memory", "remainingVms", "isSoldOut"] {
        if fields.get(name).is_none_or(|value| value.get() == "null") {
            return Err(invalid(op, 200));
        }
    }
    let metadata = fields
        .remove("metaData")
        .and_then(|value| serde_json::from_str::<String>(value.get()).ok())
        .unwrap_or_default();
    let body = serde_json::to_string(&fields).map_err(|_| invalid(op, 200))?;
    let Field(mut flavor): Field<Flavor> =
        serde_json::from_str(&body).map_err(|_| invalid(op, 200))?;
    flavor.metadata = metadata;
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
