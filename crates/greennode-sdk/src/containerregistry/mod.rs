//! Read container registry repositories and permissions.

mod reads;
pub use reads::*;

use crate::config::Session;
use crate::wire::product::{
    operation::{self, Operation, Output},
    redirect::{Classification, Retry},
    retry,
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Config, Error, ErrorClass, Failure, List, PagedList, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;

/// Client for token-scoped container registry reads.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

/// Selects repository filters.
#[derive(Clone, Debug, Default)]
pub struct ListRepositoriesInput {
    /// Access level, defaulting to `ALL` when empty.
    pub access_level: String,
    /// Name filter sent to the service.
    pub name: String,
}

/// Repository list and pagination metadata.
pub type ListRepositoriesOutput = PagedList<Repository>;

/// Selects all available permissions.
#[derive(Clone, Debug, Default)]
pub struct ListPermissionsInput {}

/// Available registry permissions.
pub type ListPermissionsOutput = List<Permission>;

/// A container registry repository.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Repository {
    /// Repository identifier.
    #[serde(rename = "uuid", deserialize_with = "null_default")]
    pub id: String,
    /// Repository name.
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Backend repository name.
    #[serde(deserialize_with = "null_default")]
    pub backend_name: String,
    /// Repository visibility.
    #[serde(deserialize_with = "null_default")]
    pub access_level: String,
    /// Registry address.
    #[serde(rename = "registryUrl", deserialize_with = "null_default")]
    pub registry_url: String,
    /// Quota in gigabytes.
    #[serde(rename = "quotaLimit", deserialize_with = "null_default")]
    pub quota_limit_gb: i64,
    /// Used quota.
    #[serde(deserialize_with = "null_default")]
    pub quota_used: f64,
    /// Number of images.
    #[serde(deserialize_with = "null_default")]
    pub image_count: i64,
    /// Number of attached users.
    #[serde(rename = "attachedUser", deserialize_with = "null_default")]
    pub attached_users: i64,
    /// Creation timestamp.
    #[serde(deserialize_with = "null_default")]
    pub created_at: String,
}

/// A registry permission and its policy identifier.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Permission {
    /// Permission identifier.
    #[serde(rename = "uuid", deserialize_with = "null_default")]
    pub id: String,
    /// Permitted action.
    #[serde(rename = "action", deserialize_with = "null_default")]
    pub action: String,
}

fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

const fn field(name: &'static str) -> ShapeField<'static> {
    ShapeField {
        name,
        shape: Shape::Any,
    }
}
const REPOSITORY: Shape<'static> = Shape::Object(&[
    field("uuid"),
    field("name"),
    field("backendName"),
    field("accessLevel"),
    field("registryUrl"),
    field("quotaLimit"),
    field("quotaUsed"),
    field("imageCount"),
    field("attachedUser"),
    field("createdAt"),
]);
const PERMISSION: Shape<'static> = Shape::Object(&[field("uuid"), field("action")]);

#[derive(Default, Deserialize)]
#[serde(transparent)]
struct Permissions(Vec<Permission>);
impl Output for Permissions {
    const SCHEMA: Shape<'static> = Shape::Array(&PERMISSION);
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Repositories {
    data: Option<Vec<Repository>>,
    list_data: Option<Vec<Repository>>,
    page: i64,
    page_size: i64,
    total_page: Option<i64>,
    total_item: Option<i64>,
}
impl Output for Repositories {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "data",
            shape: Shape::Array(&REPOSITORY),
        },
        ShapeField {
            name: "listData",
            shape: Shape::Array(&REPOSITORY),
        },
        field("page"),
        field("pageSize"),
        field("totalPage"),
        field("totalItem"),
    ]);
    const CHECK_RENDERED_NUMBERS: bool = true;
    fn decode_response(op: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let result: Self = operation::decode_response(op, response)?;
        let keys: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&response.body)
                .map_err(|_| Box::new(Error::new(ErrorClass::Api)))?;
        if keys
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("data") || key.eq_ignore_ascii_case("listData"))
            .count()
            != 1
        {
            return Err(Box::new(Error::new(ErrorClass::Api)));
        }
        let credentials = &response.credentials;
        let reflected = [
            result.page,
            result.page_size,
            result.total_page.unwrap_or_default(),
            result.total_item.unwrap_or_default(),
        ]
        .into_iter()
        .any(|v| {
            credentials.reflects(v.to_string().as_bytes())
                || crate::output_number::reflects(v as f64, credentials, false)
        }) || result
            .data
            .iter()
            .chain(result.list_data.iter())
            .flatten()
            .any(|item| {
                [item.quota_limit_gb, item.image_count, item.attached_users]
                    .into_iter()
                    .any(|v| {
                        credentials.reflects(v.to_string().as_bytes())
                            || crate::output_number::reflects(v as f64, credentials, false)
                    })
                    || crate::output_number::reflects(item.quota_used, credentials, true)
            });
        if reflected {
            return Err(Box::new(Error::new(ErrorClass::Api)));
        }
        Ok(result)
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Shares authentication and connections without performing I/O.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
        }
    }

    /// Reads all permission actions.
    pub async fn list_permissions(
        &self,
        ctx: &RequestContext,
        _input: ListPermissionsInput,
    ) -> Result<ListPermissionsOutput, Failure<ListPermissionsOutput>> {
        let items: Permissions = self
            .read(
                ctx,
                &read_operation("containerregistry.ListPermissions", "v1/user/permissions"),
                &[],
            )
            .await?;
        Ok(List { items: items.0 })
    }

    /// Reads repository summaries, returning partial output if continuation is required.
    pub async fn list_repositories(
        &self,
        ctx: &RequestContext,
        input: ListRepositoriesInput,
    ) -> Result<ListRepositoriesOutput, Failure<ListRepositoriesOutput>> {
        let access = if input.access_level.is_empty() {
            "ALL"
        } else {
            &input.access_level
        };
        let result: Repositories = self
            .read(
                ctx,
                &read_operation("containerregistry.ListRepositories", "v1/repository"),
                &[("accessLevel", access), ("name", &input.name)],
            )
            .await?;
        let (items, total_page, total_item) = match (
            result.data,
            result.list_data,
            result.total_page,
            result.total_item,
        ) {
            (Some(items), None, Some(pages), Some(total))
            | (None, Some(items), Some(pages), Some(total)) => (items, pages, total),
            _ => return Err(failure(Error::new(ErrorClass::Api))),
        };
        let len = i64::try_from(items.len()).unwrap_or(i64::MAX);
        if result.page < 0
            || result.page_size < 0
            || total_page < 0
            || total_item < 0
            || len > total_item
            || (total_page == 0 && total_item != 0)
            || (total_item == 0 && total_page > 1)
            || (total_page <= 1 && len != total_item)
        {
            return Err(failure(Error::new(ErrorClass::Api)));
        }
        let output = PagedList {
            items,
            page: result.page,
            page_size: result.page_size,
            total_page,
            total_item,
        };
        if total_page > 1 || total_item > len {
            return Err(Failure::new(Error::new(ErrorClass::Api).with_detail("containerregistry.ListRepositories: incomplete list; continuation parameters are unverified"), Some(output), WriteDispatch::NotAWrite));
        }
        Ok(output)
    }

    async fn read<T: Output, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        self.read_path(ctx, operation, &[], query).await
    }

    async fn read_path<T: Output, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let session = &self.session;
        let base = session
            .endpoints()
            .container_registry
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute::<T>(
            runtime,
            base,
            operation,
            path,
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, value)| value)
        .map_err(|error| {
            let (error, _, dispatch) = error.into_parts();
            Failure::new(error, None, dispatch)
        })
    }
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}
fn read_operation(id: &'static str, template: &'static str) -> Operation {
    Operation {
        id,
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
        sensitive: false,
        reject_reflected_credentials: true,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::test_server::{Response, Server};
    use crate::{ConfigOptions, EndpointOverrides, Secret};

    fn response(body: &[u8]) -> Response {
        Response::Http {
            status: "200 OK".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body: body.to_vec(),
        }
    }

    fn client(server: &Server) -> Client {
        let config = Config::new(
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_static_token(Secret::new("synthetic-registry-token"))
                .with_endpoint_overrides(EndpointOverrides {
                    container_registry: Some(server.origin.clone()),
                    ..Default::default()
                }),
        )
        .unwrap();
        config
            .session()
            .set_test_product_root(server.root_der.clone());
        Client::new(&config)
    }

    #[tokio::test]
    async fn list_permissions_reads_bare_array() {
        let server = Server::start(vec![response(include_bytes!(
            "../../tests/fixtures/containerregistry/list_permissions.json"
        ))]);
        let output = client(&server)
            .list_permissions(&RequestContext::new().0, ListPermissionsInput::default())
            .await
            .unwrap();
        assert!(server.heads()[0].starts_with(b"GET /v1/user/permissions "));
        assert_eq!(output.items.len(), 3);
        assert_eq!(
            serde_json::to_value(output).unwrap()["Items"][0]["uuid"],
            "policy-1"
        );
    }

    #[tokio::test]
    async fn incomplete_repository_list_returns_partial_output() {
        let server = Server::start(vec![response(
            br#"{"data":[{"uuid":"repo-1"}],"totalPage":2,"totalItem":2}"#,
        )]);
        let error = client(&server)
            .list_repositories(&RequestContext::new().0, ListRepositoriesInput::default())
            .await
            .unwrap_err();
        assert_eq!(error.partial().unwrap().items[0].id, "repo-1");
    }

    #[tokio::test]
    async fn conflicting_repository_collections_are_rejected() {
        let server = Server::start(vec![response(
            br#"{"data":[],"listData":null,"totalPage":0,"totalItem":0}"#,
        )]);
        let error = client(&server)
            .list_repositories(&RequestContext::new().0, ListRepositoriesInput::default())
            .await
            .unwrap_err();
        assert!(error.partial().is_none());
    }

    #[tokio::test]
    async fn list_repositories_reads_paged_response() {
        let server = Server::start(vec![response(include_bytes!(
            "../../tests/fixtures/containerregistry/list_repositories.json"
        ))]);
        let output = client(&server)
            .list_repositories(&RequestContext::new().0, ListRepositoriesInput::default())
            .await
            .unwrap();
        assert!(server.heads()[0].starts_with(b"GET /v1/repository?accessLevel=ALL&name= "));
        assert_eq!(output.total_item, 1);
        assert_eq!(
            serde_json::to_value(output).unwrap()["Items"][0]["uuid"],
            "repo-1"
        );
    }
}
