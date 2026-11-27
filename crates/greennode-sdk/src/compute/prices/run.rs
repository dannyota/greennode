use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::model::pricing::{DiscoveryFailure, DiscoveryMetadata, discovery_notes, note};
use crate::quote::{
    catalog::{self, Catalog, Image, VolumeType, Zone},
    input,
    request::{self, Product, Request, decode},
};
use crate::wire::product::options::PathIds;
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::super::{
    Client, Flavor, failure,
    wire::{api, invalid_input},
};
use super::{
    FlavorPrice, FlavorPriceReference, ListFlavorPricesInput, ListFlavorPricesOutput,
    catalog::stops,
};

const OP: &str = "compute.ListFlavorPrices";
const QUOTE: &str = "pricing.GetQuote";

pub(super) struct Run<'a> {
    pub(super) catalog: Catalog<'a>,
    pub(super) input: &'a ListFlavorPricesInput,
    pub(super) output: ListFlavorPricesOutput,
    pub(super) identities: HashMap<(String, String), Flavor>,
    pub(super) tuples: HashMap<String, Flavor>,
    pub(super) stopped: bool,
    errors: Vec<Error>,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes the monthly instance component of each matching reference server.
    pub async fn list_flavor_prices(
        &self,
        ctx: &RequestContext,
        input: ListFlavorPricesInput,
    ) -> Result<ListFlavorPricesOutput, Failure<ListFlavorPricesOutput>> {
        crate::wire::product::exit::finish!(async {
            validate(&input).map_err(|error| failure(*error))?;
            let mut run = Run {
                catalog: Catalog::new(&self.session, ctx, input.max_requests),
                input: &input,
                output: ListFlavorPricesOutput {
                    items: Vec::new(),
                    metadata: DiscoveryMetadata {
                        region: self.region.clone(),
                        complete: true,
                        requests: 0,
                        quoted_at: self.session.clock().utc_now(),
                        failures: Vec::new(),
                        notes: discovery_notes()
                            .into_iter()
                            .filter(|note| {
                                !matches!(
                                    note.scope.as_str(),
                                    "storage" | "object-storage" | "network"
                                )
                            })
                            .collect(),
                        excluded_groups: None,
                    },
                },
                identities: HashMap::new(),
                tuples: HashMap::new(),
                stopped: false,
                errors: Vec::new(),
            };
            run.note("visibility", "compute", "Console-family scope excludes custom, partner, and legacy catalogs unless requested. Catalog visibility establishes no sale eligibility.");
            run.note("reference-component", "compute", "Each price is the instance component of its recorded reference configuration. The root disk and other excluded costs are not included.");
            if let Err(error) = run.discover().await {
                return Err(failure(run.safe_error(*error)));
            }
            run.finish()
        })
    }
}

fn validate(input: &ListFlavorPricesInput) -> Result<(), Box<Error>> {
    if input.min_ram < 0
        || input.max_ram < 0
        || (input.max_ram > 0 && input.min_ram > input.max_ram)
    {
        return Err(Box::new(invalid_input("invalid raw RAM bounds")));
    }
    if !(0..=2000).contains(&input.max_requests) {
        return Err(Box::new(invalid_input(
            "MaxRequests must be between 1 and 2000",
        )));
    }
    for (name, value, rule) in [
        ("ZoneID", &input.zone_id, PathIds::WithUnderscore),
        ("Family", &input.family, PathIds::Strict),
        ("Platform", &input.platform, PathIds::Strict),
        (
            "RootDiskTypeID",
            &input.root_disk_type_id,
            PathIds::WithUnderscore,
        ),
    ] {
        if !value.is_empty() && !rule.allows(value) {
            let pattern = if rule == PathIds::Strict {
                "^[A-Za-z0-9-]+$"
            } else {
                "^[A-Za-z0-9_-]+$"
            };
            return Err(Box::new(invalid_input(&format!(
                "{OP} requires {name} to match {pattern}"
            ))));
        }
    }
    Ok(())
}

impl Run<'_> {
    async fn discover(&mut self) -> Result<(), Box<Error>> {
        let project = self.catalog.project().await?;
        let mut zones = self.catalog.zones(&project).await?;
        let selected = &self.input.zone_id;
        if !selected.is_empty() {
            zones.retain(|zone| zone.id == *selected);
            if zones.is_empty() {
                return Err(Box::new(Error::new(ErrorClass::InvalidInput)));
            }
        }
        let platforms = self.platforms(&project).await?;
        for zone in &zones {
            if selected.is_empty() && zone.enabled == Some(false) {
                self.note(
                    "inapplicable",
                    &zone.id,
                    "The zone is disabled; discovery retains its catalog rows without quotes.",
                );
            }
            if let Err(error) = self.console(&project, zone, &platforms).await {
                self.record(OP, &zone.id, error)?;
            }
            if self.stopped {
                return Ok(());
            }
        }
        if self.input.include_non_public
            && let Err(error) = self.non_public(&project, &zones).await
        {
            self.record(OP, "non-public", error)?;
        }
        if self.stopped {
            return Ok(());
        }
        if self.output.items.is_empty() && self.errors.is_empty() {
            self.note(
                "unmatched-scope",
                "compute",
                "No supported flavor matches the requested catalog scope and exact filters.",
            );
        }
        let custom = &self.input.root_disk_type_id;
        let has_rows = self
            .output
            .items
            .iter()
            .any(|row| row.price.status != "inapplicable");
        if !has_rows && custom.is_empty() {
            return Ok(());
        }
        let mut disks = match self.catalog.volume_catalog(&project).await {
            Ok(disks) => disks,
            Err(error) => {
                self.record(catalog::TYPES, "reference-disks", error)?;
                return Ok(());
            }
        };
        if !custom.is_empty() {
            disks.retain(|disk| disk.quote_id() == custom);
            if disks.is_empty() {
                return Err(Box::new(invalid_input("unknown RootDiskTypeID")));
            }
        }
        if !has_rows {
            return Ok(());
        }
        for zone in &zones {
            if selected.is_empty() && zone.enabled == Some(false) {
                continue;
            }
            self.references(&project, zone, &disks).await?;
            if self.stopped {
                return Ok(());
            }
        }
        self.quotes().await
    }

    async fn references(
        &mut self,
        project: &str,
        zone: &Zone,
        disks: &[VolumeType],
    ) -> Result<(), Box<Error>> {
        let images = match self.catalog.images(project, &zone.id).await {
            Ok(images) => images,
            Err(error) => return self.record(catalog::IMAGES, &zone.id, error),
        };
        let custom = !self.input.root_disk_type_id.is_empty();
        for index in 0..self.output.items.len() {
            let row = &mut self.output.items[index];
            if row.zone_id != zone.id {
                continue;
            }
            let selected = reference(&row.flavor, &images, disks, custom, || {
                self.catalog.preflight(0)
            });
            let selected = selected.and_then(|selection| {
                if let Reference::Found(candidate) = &selection {
                    self.catalog
                        .rows_with_bytes(0, candidate.retained_bytes())?;
                }
                Ok(selection)
            });
            match selected {
                Ok(Reference::Found(candidate)) => row.reference = Some(candidate.into_owned()),
                Ok(Reference::Unavailable) => {
                    let key = row.key();
                    let message = if custom {
                        "No supported unlicensed Ubuntu image and bounded root disk pair is available."
                    } else {
                        "No supported unlicensed Ubuntu image and bounded 3000 IOPS root pair is available."
                    };
                    self.note("reference-unavailable", &key, message);
                }
                Ok(Reference::OutsideBounds) => {
                    row.price.status = "inapplicable".into();
                    let key = row.key();
                    self.note(
                        "inapplicable",
                        &key,
                        "The reference root disk size exceeds the selected type's maximum.",
                    );
                }
                Err(error) => {
                    let key = row.key();
                    self.record(OP, &key, error)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn quotes(&mut self) -> Result<(), Box<Error>> {
        let mut lookups = HashMap::new();
        let mut groups: Vec<(Value, Vec<usize>)> = Vec::new();
        for (index, row) in self.output.items.iter().enumerate() {
            if let Some(reference) = row.reference.as_ref() {
                let body = input::server(input::Server {
                    zone: &row.zone_id,
                    flavor: &row.flavor.flavor_id,
                    image: &reference.image_id,
                    root_size: reference.root_disk_size_gb,
                    root_type: &reference.root_disk_type_id,
                    data_size: 0,
                    data_type: "",
                    encrypted: false,
                });
                let slot = *lookups.entry(body.to_string()).or_insert_with(|| {
                    let pos = groups.len();
                    groups.push((body, Vec::new()));
                    pos
                });
                if let Some(group) = groups.get_mut(slot) {
                    group.1.push(index);
                }
            }
        }
        if let Err(error) = self.catalog.preflight(groups.len()) {
            return self.record(OP, "quotes", error);
        }
        let custom = !self.input.root_disk_type_id.is_empty();
        for (body, indices) in groups {
            let result = self
                .catalog
                .exchange(Request {
                    operation: QUOTE,
                    template: "v1/price",
                    product: Product::Portal,
                    path: &[],
                    query: &[],
                    body: Some(&body),
                })
                .await
                .and_then(|raw| {
                    let value = catalog::value(QUOTE, &raw)?;
                    let price = decode::instance(QUOTE, &value)?;
                    if custom && !valid_root_quote(&value) {
                        return Err(request::invalid(QUOTE, 200));
                    }
                    self.catalog.rows(0)?;
                    Ok(price)
                });
            match result {
                Ok(price) => {
                    for index in indices {
                        if let Some(row) = self.output.items.get_mut(index) {
                            row.price = price.clone();
                            if row.flavor.memory > 0 {
                                row.price_per_ram_unit = price
                                    .monthly_price
                                    .map(|value| value / row.flavor.memory as f64);
                            }
                        }
                    }
                }
                Err(error) => {
                    if !self.input.allow_partial {
                        return Err(error);
                    }
                    for index in indices {
                        if let Some(row) = self.output.items.get_mut(index) {
                            row.price.status = "failed".into();
                            let key = row.key();
                            self.record_metadata(QUOTE, &key, &error);
                        }
                    }
                    self.stopped = stops(&error);
                    self.errors.push(*error);
                }
            }
            if self.stopped {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn note(&mut self, topic: &str, scope: &str, text: &str) {
        self.output
            .metadata
            .notes
            .push(note(topic, scope, text, "", "observed"));
    }

    pub(super) fn record(
        &mut self,
        operation: &'static str,
        key: &str,
        error: Box<Error>,
    ) -> Result<(), Box<Error>> {
        if !self.input.allow_partial {
            return Err(error);
        }
        self.record_metadata(operation, key, &error);
        self.stopped = stops(&error);
        self.errors.push(*error);
        Ok(())
    }

    fn record_metadata(&mut self, operation: &'static str, key: &str, error: &Error) {
        let api = error.api_error();
        let status = api.and_then(crate::ApiError::status);
        self.output.metadata.complete = false;
        self.output.metadata.failures.push(DiscoveryFailure {
            operation: operation.into(),
            item_key: key.into(),
            status: i64::from(status.unwrap_or(0)),
            code: if api.is_some_and(|api| api.code() == "InvalidResponse") {
                "InvalidResponse"
            } else if api.is_some() {
                status_code(status).unwrap_or_default()
            } else {
                "DiscoveryIncomplete"
            }
            .into(),
        });
    }

    fn safe_error(&self, error: Error) -> Error {
        self.catalog.evidence.screen_error(error)
    }

    #[expect(
        clippy::result_large_err,
        reason = "The public API returns Failure by value"
    )]
    fn finish(mut self) -> Result<ListFlavorPricesOutput, Failure<ListFlavorPricesOutput>> {
        if self.output.items.is_empty() && !self.errors.is_empty() {
            let error = self.errors.remove(0);
            return Err(failure(self.safe_error(error)));
        }
        self.output
            .items
            .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        self.output.metadata.requests = i64::try_from(self.catalog.requests()).unwrap_or(i64::MAX);
        self.output.metadata.quoted_at = self.catalog.session.clock().utc_now();
        let evidence = &self.catalog.evidence;
        if !evidence.can_retain_partial()
            || serde_json::to_value(&self.output)
                .map_or(true, |value| evidence.output_reflects(&value))
            || self
                .output
                .items
                .iter()
                .flat_map(|row| [row.price.monthly_price, row.price_per_ram_unit])
                .flatten()
                .any(|amount| evidence.reflects_fixed_two(amount))
        {
            return Err(failure(api(
                OP,
                Some(200),
                "InvalidResponse",
                "invalid discovery response; body withheld",
                evidence,
            )));
        }
        if self.errors.is_empty() {
            return Ok(self.output);
        }
        let mut error = Error::new(ErrorClass::DiscoveryIncomplete);
        for cause in std::mem::take(&mut self.errors) {
            error = error.with_cause(self.safe_error(cause));
        }
        Err(Failure::new(
            error,
            Some(self.output),
            WriteDispatch::NotAWrite,
        ))
    }
}

impl FlavorPrice {
    pub(super) fn key(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.zone_id,
            self.flavor.flavor_zone_id,
            self.flavor.flavor_id,
            self.family,
            self.platform
        )
    }

    pub(super) fn sort_key(&self) -> (&str, &str, &str, i64, i64, &str) {
        (
            &self.zone_id,
            &self.family,
            &self.platform,
            self.flavor.memory,
            self.flavor.cpu,
            &self.flavor.flavor_id,
        )
    }
}

#[derive(Deserialize)]
struct ImageSupport {
    #[serde(rename = "imageTypeSupport")]
    image_types: Vec<String>,
}

enum Reference<'a> {
    Found(ReferenceCandidate<'a>),
    Unavailable,
    OutsideBounds,
}

struct ReferenceCandidate<'a> {
    image: &'a Image,
    disk_type_id: &'a str,
    size: i64,
}

impl ReferenceCandidate<'_> {
    fn sort_key(&self) -> (i64, &str, &str, &str) {
        (
            self.size,
            &self.image.image_version,
            &self.image.id,
            self.disk_type_id,
        )
    }

    fn retained_bytes(&self) -> usize {
        [
            self.image.id.len(),
            self.image.image_type.len(),
            self.image.image_version.len(),
            self.disk_type_id.len(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    fn into_owned(self) -> FlavorPriceReference {
        FlavorPriceReference {
            image_id: self.image.id.clone(),
            image_type: self.image.image_type.clone(),
            image_version: self.image.image_version.clone(),
            root_disk_type_id: self.disk_type_id.into(),
            root_disk_size_gb: self.size,
        }
    }
}

fn reference<'a>(
    flavor: &Flavor,
    images: &'a [Image],
    disks: &'a [VolumeType],
    custom: bool,
    mut check: impl FnMut() -> Result<(), Box<Error>>,
) -> Result<Reference<'a>, Box<Error>> {
    check()?;
    let Ok(support) = serde_json::from_str::<ImageSupport>(&flavor.metadata) else {
        return Ok(Reference::Unavailable);
    };
    if support.image_types.iter().any(String::is_empty) {
        return Ok(Reference::Unavailable);
    }
    let mut best: Option<ReferenceCandidate<'_>> = None;
    let mut out_of_bounds = false;
    for image in images {
        check()?;
        if !support.image_types.contains(&image.image_type)
            || !image.unlicensed_ubuntu()
            || !image
                .minimum_cpu()
                .is_some_and(|cpu| (0..=flavor.cpu).contains(&cpu))
            || !image
                .minimum_memory()
                .is_some_and(|memory| (0..=flavor.memory).contains(&memory))
        {
            continue;
        }
        let Some(minimum) = image.root_minimum() else {
            continue;
        };
        for disk in disks {
            check()?;
            if !custom && disk.iops != 3000 {
                continue;
            }
            let size = minimum.max(disk.min_size);
            if size > disk.max_size {
                out_of_bounds = true;
                continue;
            }
            let candidate = ReferenceCandidate {
                image,
                disk_type_id: disk.quote_id(),
                size,
            };
            if best
                .as_ref()
                .is_none_or(|best| candidate.sort_key() < best.sort_key())
            {
                best = Some(candidate);
            }
        }
    }
    check()?;
    Ok(match best {
        Some(reference) => Reference::Found(reference),
        None if custom && out_of_bounds => Reference::OutsideBounds,
        None => Reference::Unavailable,
    })
}

fn valid_root_quote(value: &Value) -> bool {
    let Some(rows) = value.get("propertiesPrice").and_then(Value::as_array) else {
        return false;
    };
    rows.len() == 2
        && rows
            .iter()
            .filter(|row| row.get("name").and_then(Value::as_str) == Some("ROOT DISK"))
            .count()
            == 1
}

fn status_code(status: Option<u16>) -> Option<&'static str> {
    match status {
        Some(400) => Some("BadRequest"),
        Some(401) => Some("Unauthorized"),
        Some(403) => Some("Forbidden"),
        Some(404) => Some("NotFound"),
        Some(409) => Some("Conflict"),
        Some(429) => Some("Throttled"),
        Some(500..=599) => Some("ServerError"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reference_checks_context_inside_the_disk_loop() {
        let image = serde_json::from_str(r#"{"id":"image-1","imageType":"ubuntu","imageVersion":"1","licence":false,"packageLimit":{"cpu":1,"memory":1,"diskSize":10}}"#).unwrap();
        let flavor = super::Flavor {
            cpu: 2,
            memory: 4,
            metadata: r#"{"imageTypeSupport":["ubuntu"]}"#.into(),
            ..Default::default()
        };
        let disks = vec![
            super::VolumeType {
                id: "disk-1".into(),
                min_size: 10,
                max_size: 20,
                iops: 3000,
                ..Default::default()
            };
            4
        ];
        let mut checks = 0;
        let images = [image];
        let result = super::reference(&flavor, &images, &disks, false, || {
            checks += 1;
            if checks < 3 {
                Ok(())
            } else {
                Err(Box::new(crate::Error::new(crate::ErrorClass::Cancelled)))
            }
        });
        assert!(matches!(result, Err(error) if error.has_class(crate::ErrorClass::Cancelled)));
    }
}
