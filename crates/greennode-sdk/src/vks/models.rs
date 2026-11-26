use crate::wire::product::{
    operation::{self, Operation, Output},
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};
use serde::{Deserialize, Deserializer, Serialize};

/// Cluster inventory in the configured region.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct Cluster {
    /// Cluster identifier.
    #[serde(rename(serialize = "ID"), deserialize_with = "null_default")]
    pub id: String,
    /// Cluster name.
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Cluster description.
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    /// Last reported cluster status.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    /// Kubernetes release channel.
    #[serde(deserialize_with = "null_default")]
    pub release_channel: String,
    /// Kubernetes version.
    #[serde(deserialize_with = "null_default")]
    pub version: String,
    /// Availability-zone strategy.
    #[serde(rename(serialize = "AZStrategy"), deserialize_with = "null_default")]
    pub az_strategy: String,
    /// Creation timestamp as reported by the service.
    #[serde(deserialize_with = "null_default")]
    pub created_at: String,
    /// Last update timestamp as reported by the service.
    #[serde(deserialize_with = "null_default")]
    pub updated_at: String,
    /// Number of nodes reported in the cluster.
    #[serde(deserialize_with = "null_default")]
    pub num_nodes: i64,
    /// Whether the cluster is private.
    #[serde(deserialize_with = "null_default")]
    pub enable_private_cluster: bool,
}

/// Kubernetes version and its availability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "PascalCase", deserialize = "camelCase"))]
pub struct ClusterVersion {
    /// Kubernetes version.
    pub version: String,
    /// Whether the version is enabled.
    pub enable: bool,
    /// Release stage.
    pub stage: String,
    /// Deprecation date, or an empty string when the service omits it.
    #[serde(default, deserialize_with = "null_default")]
    pub deprecated_at: String,
}

/// Kubernetes limits and cluster usage for the account in one region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Quota {
    /// Maximum cluster count.
    pub max_clusters: i64,
    /// Current cluster count.
    pub num_clusters: i64,
    /// Maximum node groups per cluster.
    pub max_node_groups_per_cluster: i64,
    /// Maximum nodes per node group.
    pub max_nodes_per_node_group: i64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClusterPage {
    pub(super) items: Vec<Cluster>,
    pub(super) total: i64,
    pub(super) page: i32,
    pub(super) page_size: i32,
}

const fn scalar(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}

impl Output for ClusterPage {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "items",
            shape: Shape::Array(&Shape::Object(&[
                scalar("id"),
                scalar("name"),
                scalar("description"),
                scalar("status"),
                scalar("releaseChannel"),
                scalar("version"),
                scalar("azStrategy"),
                scalar("createdAt"),
                scalar("updatedAt"),
                scalar("numNodes"),
                scalar("enablePrivateCluster"),
            ])),
        },
        scalar("total"),
        scalar("page"),
        scalar("pageSize"),
    ]);

    fn decode_response(
        operation: &Operation,
        response: &Response,
    ) -> Result<Self, Box<crate::Error>> {
        decode_integer_projection(operation, response)
    }
}

#[derive(Default, Deserialize)]
pub(super) struct Versions(pub(super) Vec<ClusterVersion>);

impl Output for Versions {
    const SCHEMA: Shape<'static> = Shape::Array(&Shape::Object(&[
        scalar("version"),
        scalar("enable"),
        scalar("stage"),
        scalar("deprecatedAt"),
    ]));
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuotaResponse {
    max_clusters: i32,
    num_clusters: i32,
    max_node_groups_per_cluster: i32,
    max_nodes_per_node_group: i32,
}

impl Output for QuotaResponse {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        scalar("maxClusters"),
        scalar("numClusters"),
        scalar("maxNodeGroupsPerCluster"),
        scalar("maxNodesPerNodeGroup"),
    ]);

    fn decode_response(
        operation: &Operation,
        response: &Response,
    ) -> Result<Self, Box<crate::Error>> {
        decode_integer_projection(operation, response)
    }
}

fn decode_integer_projection<T: Output>(
    operation: &Operation,
    response: &Response,
) -> Result<T, Box<crate::Error>> {
    if operation.success.contains(&response.status)
        && response.credentials.has_numeric_product_credential()
    {
        return Err(Box::new(super::malformed(operation.id)));
    }
    operation::decode_response(operation, response)
}

impl From<QuotaResponse> for Quota {
    fn from(quota: QuotaResponse) -> Self {
        Self {
            max_clusters: i64::from(quota.max_clusters),
            num_clusters: i64::from(quota.num_clusters),
            max_node_groups_per_cluster: i64::from(quota.max_node_groups_per_cluster),
            max_nodes_per_node_group: i64::from(quota.max_nodes_per_node_group),
        }
    }
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}
