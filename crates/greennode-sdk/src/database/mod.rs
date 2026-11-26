//! Read relational database inventory and catalogs.

mod model;
#[cfg(test)]
mod tests;
mod wire;

pub use model::{EngineVersion, Flavor, Instance, Network, Page, StorageType, Subnet, Zone};

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Config, Error, ErrorClass, Failure, List, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::Serialize;
use std::sync::Arc;
use wire::{Data, Envelope, InstanceData, StorageData, valid_id};

/// Client for token-scoped relational database operations in `hcm-3`.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    supported_region: bool,
}

/// Selects the engine version catalog.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseEngineVersionsInput;
/// Engine versions available to the account.
pub type ListDatabaseEngineVersionsOutput = List<EngineVersion>;
/// Selects the zone catalog.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseZonesInput;
/// Database zones available to the account.
pub type ListDatabaseZonesOutput = List<Zone>;
/// Selects flavors for an engine and zone.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseFlavorsInput {
    /// Datastore engine type.
    pub engine: String,
    /// Datastore engine version.
    pub engine_version: String,
    /// Database zone ID.
    pub zone_id: String,
}
/// Flavors available for the selected engine and zone.
pub type ListDatabaseFlavorsOutput = List<Flavor>;
/// Selects storage types for a zone.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseStorageTypesInput {
    /// Database zone ID.
    pub zone_id: String,
}
/// Storage types and the project reported by the service.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListDatabaseStorageTypesOutput {
    /// Project ID returned by the service.
    #[serde(rename = "ProjectID")]
    pub project_id: String,
    /// Matching storage types.
    #[serde(rename = "Items")]
    pub items: Vec<StorageType>,
}
/// Selects networks for a zone.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseNetworksInput {
    /// Database zone ID.
    pub zone_id: String,
}
/// Networks available in the selected zone.
pub type ListDatabaseNetworksOutput = List<Network>;
/// Selects one page of database instances.
#[derive(Clone, Debug, Default)]
pub struct ListDatabaseInstancesInput {
    /// One-based page number; zero selects one.
    pub page_number: i32,
    /// Page size; zero selects 100.
    pub page_size: i32,
}
/// One page of database instances and its metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListDatabaseInstancesOutput {
    /// Project ID returned by the service.
    #[serde(rename = "ProjectID")]
    pub project_id: String,
    /// Instances on the requested page.
    #[serde(rename = "Items")]
    pub items: Vec<Instance>,
    /// Page metadata reported by the service.
    #[serde(rename = "Page")]
    pub page: Page,
}

/// Selects one relational instance by ID.
#[derive(Clone, Debug, Default)]
pub struct GetDatabaseInstanceInput {
    /// Exact instance ID, using ASCII letters, digits, hyphens, or underscores.
    pub id: String,
}

/// The requested relational database instance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetDatabaseInstanceOutput {
    /// Instance details returned by the service.
    #[serde(rename = "Instance")]
    pub instance: Instance,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Shares the configuration's authentication and connections without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            supported_region: config.region() == "hcm-3",
        }
    }
    /// Reads the datastore catalog.
    pub async fn list_database_engine_versions(
        &self,
        ctx: &RequestContext,
        _: ListDatabaseEngineVersionsInput,
    ) -> Result<ListDatabaseEngineVersionsOutput, Failure<ListDatabaseEngineVersionsOutput>> {
        Ok(List {
            items: self
                .read(
                    ctx,
                    &read_operation(
                        "database.ListDatabaseEngineVersions",
                        "vdb-relational/v1/database-instances/datastore",
                    ),
                    &[],
                )
                .await?,
        })
    }
    /// Reads the database zone catalog.
    pub async fn list_database_zones(
        &self,
        ctx: &RequestContext,
        _: ListDatabaseZonesInput,
    ) -> Result<ListDatabaseZonesOutput, Failure<ListDatabaseZonesOutput>> {
        Ok(List {
            items: self
                .read(
                    ctx,
                    &read_operation(
                        "database.ListDatabaseZones",
                        "vdb-relational/v1/database-instances/zones",
                    ),
                    &[],
                )
                .await?,
        })
    }
    /// Reads the flavor catalog for an engine and zone.
    pub async fn list_database_flavors(
        &self,
        ctx: &RequestContext,
        input: ListDatabaseFlavorsInput,
    ) -> Result<ListDatabaseFlavorsOutput, Failure<ListDatabaseFlavorsOutput>> {
        if !valid_id(&input.engine) || input.engine_version.is_empty() || !valid_id(&input.zone_id)
        {
            return Err(failure(invalid_input()));
        }
        let query = [
            ("type", input.engine.as_str()),
            ("version", input.engine_version.as_str()),
            ("zoneId", input.zone_id.as_str()),
        ];
        Ok(List {
            items: self
                .read(
                    ctx,
                    &read_operation(
                        "database.ListDatabaseFlavors",
                        "vdb-relational/v1/database-instances/flavors",
                    ),
                    &query,
                )
                .await?,
        })
    }
    /// Reads the storage type catalog for a zone.
    pub async fn list_database_storage_types(
        &self,
        ctx: &RequestContext,
        input: ListDatabaseStorageTypesInput,
    ) -> Result<ListDatabaseStorageTypesOutput, Failure<ListDatabaseStorageTypesOutput>> {
        if !valid_id(&input.zone_id) {
            return Err(failure(invalid_input()));
        }
        let data: StorageData = self
            .read(
                ctx,
                &read_operation(
                    "database.ListDatabaseStorageTypes",
                    "vdb-relational/v1/database-instances/volume/types",
                ),
                &[("zoneId", &input.zone_id)],
            )
            .await?;
        Ok(ListDatabaseStorageTypesOutput {
            project_id: data.project_id,
            items: data.data,
        })
    }
    /// Reads the eligible networks and their subnets for a zone.
    pub async fn list_database_networks(
        &self,
        ctx: &RequestContext,
        input: ListDatabaseNetworksInput,
    ) -> Result<ListDatabaseNetworksOutput, Failure<ListDatabaseNetworksOutput>> {
        if !valid_id(&input.zone_id) {
            return Err(failure(invalid_input()));
        }
        Ok(List {
            items: self
                .read(
                    ctx,
                    &read_operation(
                        "database.ListDatabaseNetworks",
                        "vdb-relational/v1/database-instances/networks/subnets",
                    ),
                    &[("zoneId", &input.zone_id)],
                )
                .await?,
        })
    }
    /// Reads one page of relational database instances.
    pub async fn list_database_instances(
        &self,
        ctx: &RequestContext,
        input: ListDatabaseInstancesInput,
    ) -> Result<ListDatabaseInstancesOutput, Failure<ListDatabaseInstancesOutput>> {
        if input.page_number < 0 || input.page_size < 0 {
            return Err(failure(invalid_input()));
        }
        let page = input.page_number.max(1).to_string();
        let size = if input.page_size == 0 {
            100
        } else {
            input.page_size
        }
        .to_string();
        let data: InstanceData = self
            .read(
                ctx,
                &read_operation(
                    "database.ListDatabaseInstances",
                    "vdb-relational/v1/database-instances",
                ),
                &[("pageNumber", &page), ("pageSize", &size)],
            )
            .await?;
        Ok(ListDatabaseInstancesOutput {
            project_id: data.project_id,
            items: data.data,
            page: data.page_object,
        })
    }
    /// Reads an instance by exact ID and checks the returned ID.
    pub async fn get_database_instance(
        &self,
        ctx: &RequestContext,
        input: GetDatabaseInstanceInput,
    ) -> Result<GetDatabaseInstanceOutput, Failure<GetDatabaseInstanceOutput>> {
        if !valid_id(&input.id) {
            return Err(failure(invalid_input()));
        }
        let operation = read_operation(
            "database.GetDatabaseInstance",
            "vdb-relational/v1/database-instances/id/{}",
        );
        let instance: Instance = self.read_path(ctx, &operation, &[&input.id], &[]).await?;
        if instance.id != input.id {
            return Err(failure(wire::invalid_response(&operation)));
        }
        Ok(GetDatabaseInstanceOutput { instance })
    }

    async fn read<T: Data, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        self.read_path(ctx, operation, &[], query).await
    }

    async fn read_path<T: Data, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let base = self
            .session
            .endpoints()
            .database
            .as_ref()
            .filter(|_| self.supported_region)
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute_with_evidence::<Envelope<T>>(
            runtime,
            base,
            operation,
            &Options {
                path_ids: PathIds::WithUnderscore,
                ..Options::DEFAULT
            },
            path,
            query,
            self.session.bearer_source(),
            bytes::Bytes::new(),
            &mut Evidence::new(),
        )
        .await
        .map(|(_, envelope)| envelope.data)
        .map_err(|error| {
            let (error, _, dispatch) = error.into_parts();
            Failure::new(wire::safe_error(operation, error), None, dispatch)
        })
    }
}
fn invalid_input() -> Error {
    Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(ApiError::without_fallback(
        "",
        None,
        SafeText::new("InvalidUsage", &[]),
        SafeText::new("vngcloud: invalid input: invalid database selection", &[]),
        false,
    )))
}
fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}
fn read_operation(id: &'static str, template: &'static str) -> Operation {
    Operation {
        id: id.strip_prefix("database.").unwrap_or(id),
        template,
        method: Method::GET,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: &[StatusCode::OK],
        cap: 8 << 20,
        allow_recovery: true,
        allow_empty: false,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        sensitive: true,
        reject_reflected_credentials: true,
        no_redirect: true,
        retry_config: retry::Config::default(),
    }
}
