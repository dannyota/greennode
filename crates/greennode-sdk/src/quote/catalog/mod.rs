mod fields;
pub(crate) mod images;
pub(crate) mod storage;
pub(crate) mod volume;

pub(crate) use fields::{array, array_rows, envelope_fields, exact, record, unpaged, wire_field};
pub(crate) use images::Image;
pub(crate) use storage::{
    StorageCatalog, StorageRegion, storage_data, storage_region, storage_region_name,
};
pub(crate) use volume::{Encryption, VolumeType};

use super::request::{self, Product, Request};
use crate::config::session::Session;
use crate::discovery::InventoryLimits;
use crate::wire::product::evidence::Evidence;
use crate::wire::product::options::{Pacing, PathIds};
use crate::{Error, ErrorClass, RequestContext};
use serde_json::{Value, value::RawValue};
use std::collections::HashSet;
use std::time::Duration;

pub(crate) const ZONES: &str = "portal.ListZones";
pub(crate) const PARENTS: &str = "volume.ListVolumeTypeZones";
pub(crate) const TYPES: &str = "volume.ListVolumeTypes";
pub(crate) const ENCRYPTIONS: &str = "volume.ListEncryptionTypes";
pub(crate) const IMAGES: &str = "compute.ListOSImages";
pub(crate) const REGIONS: &str = "storage.ListRegions";
const RETAINED_BYTE_LIMIT: usize = 32 << 20;
const ROW_LIMIT: usize = 10_000;

pub(crate) struct Catalog<'a> {
    pub(crate) session: &'a Session,
    ctx: RequestContext,
    pub(crate) evidence: Evidence,
    budget: Budget,
    unaccepted_bytes: usize,
}

struct Budget {
    max: usize,
    requests: usize,
    rows: usize,
    retained: usize,
    byte_limit: usize,
    pacing: Pacing,
}

impl Budget {
    fn preflight(&self, count: usize) -> Result<(), Box<Error>> {
        if count > self.max.saturating_sub(self.requests) {
            return Err(limit(format!(
                "discovery limit exceeded: requires {} requests, maximum {}",
                self.requests.saturating_add(count),
                self.max,
            )));
        }
        Ok(())
    }
}

impl InventoryLimits for Budget {
    fn check_bytes(&self, bytes: usize) -> Result<(), Box<Error>> {
        if bytes > self.byte_limit.saturating_sub(self.retained) {
            return Err(limit(
                "discovery limit exceeded: maximum 32 MiB retained responses",
            ));
        }
        Ok(())
    }

    fn reserve(&mut self) -> Result<(), Box<Error>> {
        self.preflight(1)?;
        self.requests += 1;
        Ok(())
    }

    fn account(&mut self, rows: usize, bytes: usize) -> Result<(), Box<Error>> {
        self.check_bytes(bytes)?;
        if rows > ROW_LIMIT.saturating_sub(self.rows) {
            return Err(limit("discovery limit exceeded: maximum 10000 rows"));
        }
        self.rows += rows;
        self.retained += bytes;
        Ok(())
    }

    fn pacing(&self) -> &Pacing {
        &self.pacing
    }
}

impl<'a> Catalog<'a> {
    pub(crate) fn new(session: &'a Session, ctx: &RequestContext, max: i64) -> Self {
        let clock = session.clock();
        let byte_limit = session.retained_byte_limit(RETAINED_BYTE_LIMIT);
        Self {
            session,
            ctx: ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(600)),
            evidence: Evidence::new(),
            unaccepted_bytes: 0,
            budget: Budget {
                max: if max == 0 {
                    500
                } else {
                    usize::try_from(max).unwrap_or(0)
                },
                requests: 0,
                rows: 0,
                retained: 0,
                byte_limit,
                pacing: Pacing::new(Duration::from_millis(500)),
            },
        }
    }

    pub(crate) fn requests(&self) -> usize {
        self.budget.requests
    }

    pub(crate) fn preflight(&self, count: usize) -> Result<(), Box<Error>> {
        self.check()?;
        self.budget.preflight(count)
    }

    pub(crate) fn rows(&mut self, count: usize) -> Result<(), Box<Error>> {
        self.rows_with_bytes(count, 0)
    }

    pub(crate) fn rows_with_bytes(
        &mut self,
        count: usize,
        retained_bytes: usize,
    ) -> Result<(), Box<Error>> {
        self.check()?;
        self.budget.account(
            count,
            retained_bytes.saturating_add(std::mem::take(&mut self.unaccepted_bytes)),
        )
    }

    fn check(&self) -> Result<(), Box<Error>> {
        self.ctx
            .check(self.session.clock())
            .map_or(Ok(()), |end| Err(Box::new(end.into())))
    }

    pub(crate) async fn project(&mut self) -> Result<String, Box<Error>> {
        self.session
            .require_project_id_with_limits(&self.ctx, &mut self.evidence, &mut self.budget)
            .await
    }

    pub(crate) async fn exchange(&mut self, req: Request<'_>) -> Result<Box<RawValue>, Box<Error>> {
        let operation = req.operation;
        self.exchange_validated(req, operation).await
    }

    async fn exchange_validated(
        &mut self,
        req: Request<'_>,
        validation_op: &'static str,
    ) -> Result<Box<RawValue>, Box<Error>> {
        self.check()?;
        if req
            .path
            .iter()
            .copied()
            .chain(req.query.iter().map(|(_, value)| *value))
            .any(|value| self.evidence.text_reflects(value))
            || req
                .body
                .is_some_and(|body| self.evidence.output_reflects(body))
        {
            return Err(request::invalid(req.operation, 200));
        }
        self.budget.reserve()?;
        self.unaccepted_bytes = 0;
        let ctx = &self.ctx;
        let clock = self.session.clock();
        let budget = &self.budget;
        let (raw, raw_len) = request::exchange_validated(
            self.session,
            &self.ctx,
            &mut self.evidence,
            req,
            Some(budget.pacing()),
            validation_op,
            |raw_len| {
                if let Some(end) = ctx.check(clock) {
                    return Err(Box::new(end.into()));
                }
                budget.check_bytes(raw_len)
            },
        )
        .await?;
        self.unaccepted_bytes = raw_len;
        Ok(raw)
    }

    pub(crate) async fn regional(
        &mut self,
        operation: &'static str,
        validation_op: &'static str,
        template: &'static str,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<Value, Box<Error>> {
        let raw = self
            .exchange_validated(
                Request {
                    operation,
                    template,
                    product: Product::Regional,
                    path,
                    query,
                    body: None,
                },
                validation_op,
            )
            .await?;
        value(validation_op, &raw)
    }

    pub(crate) async fn zones(
        &mut self,
        project: &str,
        validation_op: &'static str,
    ) -> Result<Vec<Zone>, Box<Error>> {
        let raw = self
            .regional(ZONES, validation_op, "v1/{}/zones", &[project], &[])
            .await?;
        let zones: Vec<Zone> = array(
            validation_op,
            &raw,
            Some("data"),
            &["uuid", "name", "isEnabled"],
        )?;
        unique(
            validation_op,
            zones.iter().map(|zone| zone.id.as_str()),
            PathIds::Strict,
        )?;
        if zones.iter().any(|zone| zone.enabled.is_none()) {
            return Err(request::invalid(validation_op, 200));
        }
        self.rows(zones.len())?;
        Ok(zones)
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[allow(dead_code, reason = "Shared fields for service ports")]
pub(crate) struct Zone {
    #[serde(default, rename = "uuid", deserialize_with = "wire_field")]
    pub(crate) id: String,
    #[serde(default, deserialize_with = "wire_field")]
    pub(crate) name: String,
    #[serde(rename = "isEnabled")]
    pub(crate) enabled: Option<bool>,
}

pub(crate) fn value(op: &'static str, raw: &RawValue) -> Result<Value, Box<Error>> {
    fn token(raw: &RawValue) -> Result<Value, serde_json::Error> {
        let text = raw.get();
        match text.as_bytes().first() {
            Some(b'{') => {
                let entries: std::collections::BTreeMap<String, &RawValue> =
                    serde_json::from_str(text)?;
                entries
                    .into_iter()
                    .map(|(key, raw)| Ok((key, token(raw)?)))
                    .collect::<Result<_, _>>()
                    .map(Value::Object)
            }
            Some(b'[') => {
                let items: Vec<&RawValue> = serde_json::from_str(text)?;
                items
                    .into_iter()
                    .map(token)
                    .collect::<Result<_, _>>()
                    .map(Value::Array)
            }
            _ => serde_json::from_str(text),
        }
    }

    token(raw).map_err(|_| request::invalid(op, 200))
}

pub(crate) fn unique<'a>(
    op: &'static str,
    ids: impl IntoIterator<Item = &'a str>,
    rule: PathIds,
) -> Result<(), Box<Error>> {
    let mut seen = HashSet::new();
    if ids.into_iter().all(|id| rule.allows(id) && seen.insert(id)) {
        Ok(())
    } else {
        Err(request::invalid(op, 200))
    }
}

fn limit(message: impl AsRef<str>) -> Box<Error> {
    Box::new(Error::new(ErrorClass::EstimateLimit).with_local_diagnostic(message.as_ref()))
}

#[cfg(test)]
mod corpus;
