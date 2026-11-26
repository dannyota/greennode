//! Read vServer volume inventory, snapshots, and prices.

mod attachment;
mod catalog;
mod inventory;
mod model;
mod paid;
mod prices;
mod quotes;
mod snapshots;
#[cfg(test)]
mod tests;
mod wire;
mod write;

pub use attachment::*;
pub use catalog::*;
pub use inventory::*;
pub use model::*;
pub use paid::*;
pub use prices::*;
pub use quotes::*;
pub use snapshots::*;

use std::sync::Arc;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, value::RawValue};

use crate::config::Session;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    retry,
};
use crate::{Config, Error, ErrorClass, Failure, RequestContext};
use wire::{CAP, Document, ShapeResult, decode, failure, invalid, screen, shape};

/// Client for vServer volume reads and snapshot inventory.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
    pricing: crate::pricing::Client,
}

#[derive(Clone, Copy)]
struct Read {
    id: &'static str,
    route: &'static str,
    shape_error: &'static str,
    backup: bool,
}

impl Read {
    const fn vserver(id: &'static str, route: &'static str, shape_error: &'static str) -> Self {
        Self {
            id,
            route,
            shape_error,
            backup: false,
        }
    }

    const fn backup(id: &'static str, route: &'static str) -> Self {
        Self {
            id,
            route,
            shape_error: wire::SNAPSHOT_SHAPE,
            backup: true,
        }
    }
}

impl Client {
    /// Shares authentication and connections with the configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().into(),
            pricing: crate::pricing::Client::new(config),
        }
    }

    async fn project(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
    ) -> Result<String, Box<Error>> {
        self.session
            .require_project_id_with_evidence(ctx, evidence)
            .await
    }

    fn guard_body(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        body: &Value,
    ) -> Result<(), Box<Error>> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if wire::reflects(evidence, body) {
            return Err(invalid(
                "vngcloud: invalid input: request contains credential material",
            ));
        }
        Ok(())
    }

    async fn document(
        &self,
        ctx: &RequestContext,
        read: Read,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<Document, Box<Error>> {
        let endpoints = self.session.endpoints();
        let base = if read.backup {
            endpoints.vserver_backup.as_ref()
        } else {
            endpoints.vserver.as_ref()
        }
        .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        let operation = Operation {
            id: read.id,
            template: read.route,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: CAP,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: false,
            sensitive: read.backup,
            reject_reflected_credentials: false,
            no_redirect: false,
            retry_config: retry::Config::default(),
        };
        let mut jitter = || self.session.retry_jitter();
        let runtime = self.session.runtime(ctx, &mut jitter)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if path
            .iter()
            .copied()
            .chain(query.iter().map(|(_, value)| *value))
            .any(|value| evidence.text_reflects(value))
        {
            return Err(invalid(
                "vngcloud: invalid input: request contains credential material",
            ));
        }
        operation::execute_with_evidence::<Document>(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            path,
            query,
            self.session.bearer_source(),
            bytes::Bytes::new(),
            evidence,
        )
        .await
        .map(|(_, body)| body)
        .map_err(|failure| Box::new(failure.into_parts().0))
    }

    async fn read<T: Serialize>(
        &self,
        ctx: &RequestContext,
        read: Read,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
        from_wire: impl FnOnce(&RawValue) -> ShapeResult<T>,
    ) -> Result<T, Box<Error>> {
        let value = self
            .read_value(ctx, read, path, query, evidence, from_wire)
            .await?;
        screen(evidence, value, read.id)
    }

    async fn read_value<T>(
        &self,
        ctx: &RequestContext,
        read: Read,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
        from_wire: impl FnOnce(&RawValue) -> ShapeResult<T>,
    ) -> Result<T, Box<Error>> {
        let document = self.document(ctx, read, path, query, evidence).await?;
        decode(&document, from_wire).map_err(|()| shape(read.id, read.shape_error))
    }
}

fn path_id(op: &str, field: &str, id: &str) -> Result<(), Box<Error>> {
    if !PathIds::Strict.allows(id) {
        return Err(invalid(if id.is_empty() {
            format!("vngcloud: invalid input: {op} requires {field}")
        } else {
            format!("vngcloud: invalid input: {op} requires {field} to match ^[A-Za-z0-9-]+$")
        }));
    }
    Ok(())
}

fn page_query(page: i64, size: i64) -> (String, String) {
    (
        page.max(crate::DEFAULT_PAGE).to_string(),
        if size > 0 {
            size
        } else {
            crate::DEFAULT_PAGE_SIZE
        }
        .to_string(),
    )
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn output<T>(result: Result<T, Box<Error>>) -> Result<T, Failure<T>> {
    result.map_err(failure)
}
