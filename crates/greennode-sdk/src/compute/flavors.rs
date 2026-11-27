use crate::wire::product::evidence::Evidence;
use crate::{Failure, List, RequestContext};

use super::wire::{Read, ReadStyle, invalid_input};
use super::*;

const ZONES: Read = Read::new(
    "compute.ListFlavorZones",
    "v1/{}/flavor_zones/product",
    ReadStyle::Plain,
);
const FLAVORS: Read = Read::new("compute.ListFlavors", "v1/{}/{}/flavors", ReadStyle::Plain);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Lists flavor zones, optionally selecting one network zone.
    pub async fn list_flavor_zones(
        &self,
        ctx: &RequestContext,
        input: ListFlavorZonesInput,
    ) -> Result<ListFlavorZonesOutput, Failure<ListFlavorZonesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            let zones = self
                .flavor_zones(ctx, &input.zone_id, &mut evidence)
                .await?;
            Ok(List {
                items: ZONES.retain(zones, &evidence)?,
            })
        })
    }

    /// Lists flavors in the selected flavor zone or throughout the selected network zone.
    pub async fn list_flavors(
        &self,
        ctx: &RequestContext,
        input: ListFlavorsInput,
    ) -> Result<ListFlavorsOutput, Failure<ListFlavorsOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.flavor_zone_id.is_empty() == input.zone_id.is_empty() {
                return Err(failure(invalid_input(
                    "compute.ListFlavors requires exactly one of FlavorZoneID and ZoneID",
                )));
            }
            let mut evidence = Evidence::new();
            let ids = if input.flavor_zone_id.is_empty() {
                self.flavor_zones(ctx, &input.zone_id, &mut evidence)
                    .await?
                    .into_iter()
                    .map(|zone| zone.id)
                    .collect()
            } else {
                vec![input.flavor_zone_id]
            };
            let mut items = Vec::new();
            for id in ids {
                FLAVORS.path("FlavorZoneID", &id).map_err(failure)?;
                let rows = self.read(ctx, &FLAVORS, &[&id], &[], &mut evidence).await?;
                let flavors: Vec<Flavor> = rows.field("flavors", &FLAVORS, &evidence)?;
                for mut flavor in flavors
                    .into_iter()
                    .filter(|row| input.name.is_empty() || row.name == input.name)
                {
                    if flavor.flavor_zone_id.is_empty() {
                        flavor.flavor_zone_id.clone_from(&id);
                    }
                    if flavor.zone_id.is_empty() {
                        flavor.zone_id.clone_from(&input.zone_id);
                    }
                    items.push(flavor);
                }
            }
            Ok(List {
                items: FLAVORS.retain(items, &evidence)?,
            })
        })
    }

    async fn flavor_zones<O>(
        &self,
        ctx: &RequestContext,
        zone: &str,
        evidence: &mut Evidence,
    ) -> Result<Vec<FlavorZone>, Failure<O>> {
        let envelope = self.read(ctx, &ZONES, &[], &[], evidence).await?;
        let mut rows: Vec<FlavorZone> = envelope.field("flavorZones", &ZONES, evidence)?;
        if !zone.is_empty() {
            rows.retain(|row| row.zone_id == zone);
        }
        Ok(rows)
    }
}
