use super::{Cache, OP, QuoteRequest, Run, Shared, invalid_input};
use crate::Error;
use crate::pricing::{EstimateCoverage, EstimateServer, EstimateStorage, EstimateVolume};
use crate::quote::{
    catalog::{
        IMAGES, REGIONS, StorageRegion, VolumeType, ZONES, storage_region, storage_region_name,
    },
    input,
};
use std::{future::Future, sync::Arc};

async fn cached<T: Clone>(
    slot: &mut Cache<T>,
    load: impl Future<Output = Result<T, Box<Error>>>,
) -> Shared<T> {
    if let Some(value) = slot {
        return value.clone();
    }
    let value = load.await.map_err(Arc::from);
    *slot = Some(value.clone());
    value
}

fn invalid(field: &'static str) -> Arc<Error> {
    Arc::new(invalid_input(field))
}

impl Run<'_> {
    fn screen_cached(&self, operation: &'static str, value: &impl serde::Serialize) -> Shared<()> {
        if self.catalog.evidence.serialized_output_reflects(value) {
            return Err(Arc::from(crate::quote::request::invalid(operation, 200)));
        }
        Ok(())
    }

    async fn project(&mut self) -> Shared<String> {
        cached(&mut self.memo.project, self.catalog.project()).await
    }

    async fn disks(&mut self, zone: &str) -> Shared<Vec<VolumeType>> {
        let project = self.project().await?;
        let zones = cached(&mut self.memo.zones, self.catalog.zones(&project)).await?;
        self.screen_cached(ZONES, &zones)?;
        if !zones.iter().any(|row| row.id == zone) {
            return Err(invalid("ZoneID"));
        }
        let volumes = cached(
            &mut self.memo.volumes,
            self.catalog.volume_catalog(&project),
        )
        .await?;
        self.screen_cached(crate::quote::catalog::TYPES, &volumes)?;
        Ok(volumes)
    }

    async fn encryption(&mut self, ids: &[&str]) -> Shared<()> {
        if ids.iter().all(|id| id.is_empty()) {
            return Ok(());
        }
        let project = self.project().await?;
        let keys = cached(
            &mut self.memo.encryption,
            self.catalog.encryptions(&project),
        )
        .await?;
        self.screen_cached(crate::quote::catalog::ENCRYPTIONS, &keys)?;
        if keys
            .iter()
            .any(|entry| self.catalog.evidence.id_reflects(&entry.key))
        {
            return Err(Arc::from(crate::quote::request::invalid(
                crate::quote::catalog::ENCRYPTIONS,
                200,
            )));
        }
        if ids
            .iter()
            .any(|id| !id.is_empty() && !keys.iter().any(|entry| entry.key == *id))
        {
            return Err(invalid("encryption type"));
        }
        Ok(())
    }

    async fn server(&mut self, row: &EstimateServer) -> Shared<Option<QuoteRequest>> {
        let disks = self.disks(&row.zone_id).await?;
        disk(&disks, &row.root_disk_type_id, row.root_disk_size)?;
        if row.data_disk_size > 0 {
            disk(&disks, &row.data_disk_type_id, row.data_disk_size)?;
        }
        self.encryption(&[
            &row.root_disk_encryption_type_id,
            &row.data_disk_encryption_type_id,
        ])
        .await?;
        let project = self.project().await?;
        let images = cached(
            self.memo.images.entry(row.zone_id.clone()).or_default(),
            self.catalog.images(&project, &row.zone_id),
        )
        .await?;
        self.screen_cached(IMAGES, &images)?;
        let image = images
            .iter()
            .find(|image| image.id == row.image_id)
            .ok_or_else(|| invalid("ImageID"))?;
        let Some(minimum) = image.root_minimum().filter(|_| image.unlicensed_ubuntu()) else {
            return Ok(None);
        };
        if row.root_disk_size < minimum {
            return Err(invalid("image root minimum"));
        }
        Ok(Some(QuoteRequest::regional(input::server(input::Server {
            zone: &row.zone_id,
            flavor: &row.flavor_id,
            image: &row.image_id,
            root_size: row.root_disk_size,
            root_type: &row.root_disk_type_id,
            data_size: row.data_disk_size,
            data_type: &row.data_disk_type_id,
            encrypted: !row.root_disk_encryption_type_id.is_empty()
                || !row.data_disk_encryption_type_id.is_empty(),
        }))))
    }

    async fn volume(&mut self, row: &EstimateVolume) -> Shared<QuoteRequest> {
        let disks = self.disks(&row.zone_id).await?;
        disk(&disks, &row.volume_type_id, row.size)?;
        self.encryption(&[&row.encryption_type_id]).await?;
        Ok(QuoteRequest::regional(input::volume(
            &row.zone_id,
            &row.volume_type_id,
            row.size,
            &row.encryption_type_id,
        )))
    }

    async fn storage(&mut self, index: usize, row: &EstimateStorage) -> Shared<QuoteRequest> {
        let name = storage_region_name(self.region, &row.region)
            .ok_or_else(|| invalid("storage region"))?;
        let regions = cached(&mut self.memo.regions, self.catalog.storage_regions()).await?;
        self.screen_cached(REGIONS, &regions)?;
        let StorageRegion {
            region_id,
            region_name,
        } = storage_region(&regions, &name).ok_or_else(|| invalid("storage region"))?;
        if let Some(item) = self.output.items.get_mut(index) {
            item.region = region_name.to_uppercase();
        }
        if self.memo.storage.len() >= 32 && !self.memo.storage.contains_key(region_id) {
            return Err(invalid("storage region count"));
        }
        let catalog = cached(
            self.memo.storage.entry(region_id.clone()).or_default(),
            self.catalog.storage_catalog(OP, region_id),
        )
        .await?;
        self.screen_cached(OP, &catalog)?;
        let (purchase, project) = catalog
            .resolve(&row.r#type, row.quota_gb)
            .ok_or_else(|| invalid("storage type or quota"))?;
        Ok(QuoteRequest {
            storage: Some(region_id.clone()),
            body: input::storage(row.quota_gb, purchase, project),
        })
    }

    fn unknown(&mut self, index: usize, reason: &str) {
        if let Some(item) = self.output.items.get_mut(index) {
            item.price.status = "unknown".into();
            self.output.coverage.push(EstimateCoverage {
                topic: item.kind.clone(),
                scope: item.key.clone(),
                status: "unknown".into(),
                reason: reason.into(),
            });
        }
    }

    pub(super) async fn prepare(&mut self) -> Shared<Vec<Option<QuoteRequest>>> {
        enum Row<'a> {
            Server(&'a EstimateServer),
            Volume(&'a EstimateVolume),
            Ip(&'a crate::pricing::EstimatePublicIp),
            Storage(&'a EstimateStorage),
        }
        let rows = self
            .input
            .servers
            .iter()
            .map(Row::Server)
            .chain(self.input.volumes.iter().map(Row::Volume))
            .chain(self.input.public_ips.iter().map(Row::Ip))
            .chain(self.input.storage.iter().map(Row::Storage));
        let mut requests = Vec::with_capacity(self.output.items.len());
        for (index, row) in rows.enumerate() {
            if self.stopped {
                break;
            }
            let (key, result) = match row {
                Row::Server(row) => {
                    let result = self.server(row).await;
                    if matches!(result, Ok(None)) {
                        self.unknown(index, "image-price-unverified");
                    }
                    (&row.key, result)
                }
                Row::Volume(row) => (&row.key, self.volume(row).await.map(Some)),
                Row::Ip(row) if row.kind == "public-vip" => (
                    &row.key,
                    self.project()
                        .await
                        .map(|_| Some(QuoteRequest::regional(input::public_ip(&row.r#type)))),
                ),
                Row::Ip(row) => {
                    self.unknown(index, "ip-price-unverified");
                    (&row.key, Ok(None))
                }
                Row::Storage(row) => (&row.key, self.storage(index, row).await.map(Some)),
            };
            match result {
                Ok(request) => requests.push(request),
                Err(error) => {
                    self.handle(key, error)?;
                    requests.push(None);
                }
            }
        }
        Ok(requests)
    }
}

fn disk(types: &[VolumeType], id: &str, size: i64) -> Shared<()> {
    let typ = types
        .iter()
        .find(|typ| typ.quote_id() == id)
        .ok_or_else(|| invalid("disk type"))?;
    if !(typ.min_size..=typ.max_size).contains(&size) {
        return Err(invalid("disk size"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::EstimateInput;
    use crate::quote::catalog::{Catalog, Zone};
    use crate::{Config, ConfigOptions, RequestContext, Secret};

    #[tokio::test]
    async fn estimate_rechecks_cached_zone_before_using_it_again() {
        let config = Config::new(
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_static_token(Secret::new("toy-setup")),
        )
        .unwrap();
        let input = EstimateInput::default();
        let (ctx, _) = RequestContext::new();
        let mut run = Run {
            input: &input,
            catalog: Catalog::new(config.session(), &ctx, 10),
            region: "hcm-3",
            memo: super::super::Memo {
                project: Some(Ok("project-1".into())),
                zones: Some(Ok(vec![Zone {
                    id: "zone-1".into(),
                    name: "pre6A6B6Cpost".into(),
                    enabled: Some(true),
                }])),
                volumes: Some(Ok(Vec::new())),
                ..Default::default()
            },
            output: super::super::accounting::initialize(
                &input,
                "hcm-3",
                time::OffsetDateTime::UNIX_EPOCH,
            ),
            errors: Vec::new(),
            stopped: false,
        };
        assert!(run.disks("zone-1").await.is_ok());
        run.catalog
            .evidence
            .add_bearer(&Secret::new("pre6A6B6Cpost"));
        let result = run.disks("zone-1").await;
        let Err(error) = result else {
            panic!("retained zone reused after a later credential")
        };
        assert_eq!(error.api_error().unwrap().code(), "InvalidResponse");
        assert_eq!(run.catalog.requests(), 0);
    }
}
