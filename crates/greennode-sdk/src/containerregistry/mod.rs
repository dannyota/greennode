//! Manage container registry repositories, users, and permissions.

mod reads;
pub use reads::*;

mod repository_writes;
mod user_writes;
pub use repository_writes::*;
pub use user_writes::*;

use crate::config::Session;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    retry,
    sender::Response,
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Config, Error, ErrorClass, Failure, List, PagedList, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;

/// Client for token-scoped container registry operations.
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
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct Repository {
    /// Repository identifier.
    #[serde(
        rename(deserialize = "uuid", serialize = "ID"),
        deserialize_with = "null_default"
    )]
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
    #[serde(
        rename(deserialize = "registryUrl", serialize = "RegistryURL"),
        deserialize_with = "null_default"
    )]
    pub registry_url: String,
    /// Quota in gigabytes.
    #[serde(
        rename(deserialize = "quotaLimit", serialize = "QuotaLimitGB"),
        deserialize_with = "null_default"
    )]
    pub quota_limit_gb: i64,
    /// Used quota.
    #[serde(deserialize_with = "null_default")]
    pub quota_used: f64,
    /// Number of images.
    #[serde(deserialize_with = "null_default")]
    pub image_count: i64,
    /// Number of attached users.
    #[serde(
        rename(deserialize = "attachedUser", serialize = "AttachedUsers"),
        deserialize_with = "null_default"
    )]
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
    #[serde(
        rename(deserialize = "uuid", serialize = "ID"),
        deserialize_with = "null_default"
    )]
    pub id: String,
    /// Permitted action.
    #[serde(
        rename(deserialize = "action", serialize = "Action"),
        deserialize_with = "null_default"
    )]
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
    #[serde(deserialize_with = "null_default")]
    page: i64,
    #[serde(deserialize_with = "null_default")]
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
        .any(|v| credentials.integer_output_reflects(v))
            || result
                .data
                .iter()
                .chain(result.list_data.iter())
                .flatten()
                .any(|item| {
                    [item.quota_limit_gb, item.image_count, item.attached_users]
                        .into_iter()
                        .any(|v| credentials.integer_output_reflects(v))
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
        self.request(
            ctx,
            operation,
            path,
            query,
            bytes::Bytes::new(),
            &mut Evidence::new(),
        )
        .await
    }

    async fn request<T: Output, O>(
        &self,
        ctx: &RequestContext,
        operation: &Operation,
        path: &[&str],
        query: &[(&str, &str)],
        body: bytes::Bytes,
        evidence: &mut Evidence,
    ) -> Result<T, Failure<O>> {
        let dispatch = if matches!(operation.class, Classification::Write) {
            WriteDispatch::NotSent
        } else {
            WriteDispatch::NotAWrite
        };
        let session = &self.session;
        let base = session
            .endpoints()
            .container_registry
            .as_ref()
            .ok_or_else(|| Failure::new(Error::new(ErrorClass::InvalidConfig), None, dispatch))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| Failure::new(*error, None, dispatch))?;
        operation::execute_with_evidence::<RegistryResponse<T>>(
            runtime,
            base,
            operation,
            &Options::DEFAULT,
            path,
            query,
            session.bearer_source(),
            body,
            evidence,
        )
        .await
        .map(|(_, response)| response.0)
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            let error = if operation.sensitive {
                withhold_registry_error(operation, error)
            } else {
                error
            };
            Failure::new(error, None, dispatch)
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
struct RegistryResponse<T>(T);

impl<T: Output> Output for RegistryResponse<T> {
    const SCHEMA: Shape<'static> = T::SCHEMA;

    fn credential_error(
        op: &Operation,
        sent: &crate::wire::product::sender::SentCredentials,
    ) -> Option<Box<Error>> {
        T::credential_error(op, sent)
    }

    fn reflected_response(_: &Operation, _: &Response) -> Box<Error> {
        Box::new(registry_error(
            "",
            None,
            "",
            "containerregistry: unsafe registry response",
        ))
    }

    fn decode_response(op: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if matches!(op.class, Classification::Read) && !op.sensitive {
            return T::decode_response(op, response).map(Self);
        }
        if op.allow_empty && response.body.is_empty() && op.success.contains(&response.status) {
            return Ok(Self::default());
        }
        T::decode_response(op, response).map(Self).map_err(|error| {
            if !op.success.contains(&response.status) {
                return error;
            }
            let malformed =
                serde_json::from_slice::<&serde_json::value::RawValue>(&response.body).is_err();
            let (operation, message) = if malformed && op.sensitive {
                (op.id, "invalid JSON response; body withheld")
            } else if op.sensitive {
                (op.id, "response failed to decode; body withheld")
            } else if malformed {
                (op.id, "invalid JSON response")
            } else {
                (op.id, "response failed to decode")
            };
            Box::new(registry_error(operation, None, "", message).with_cause(*error))
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
        reflection: crate::wire::product::operation::Reflection::Refuse,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}

/// Service-specific registry failure identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorDetail {
    /// The repository contains images and the delete was refused.
    RepositoryNotEmpty,
    /// The issued secret cannot be attributed to a returned user identity.
    UserNotFound,
    /// A repository delete could not establish the current image count.
    ImageCountUnknown,
    /// The SDK rejected locally invalid input.
    InvalidInput,
}

fn registry_error(
    operation: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
) -> Error {
    Error::from_api(crate::ApiError::new(
        operation,
        status,
        crate::error::SafeText::new(code, &[]),
        crate::error::SafeText::new(message, &[]),
        false,
    ))
}

fn withhold_registry_error(operation: &Operation, error: Error) -> Error {
    if error.is_body_too_large()
        || error.class() == ErrorClass::Cancelled
        || error.class() == ErrorClass::DeadlineExceeded
    {
        return error;
    }
    match error.api_error() {
        Some(api) if api.status().is_some() => Error::from_api(crate::ApiError::new(
            operation.id,
            api.status(),
            crate::error::SafeText::new(api.code(), &[]),
            crate::error::SafeText::new("registry response withheld", &[]),
            api.retryable(),
        ))
        .with_cause(error),
        _ => error,
    }
}

fn redact_failure<T>(failure: Failure<T>, evidence: &Evidence) -> Failure<T> {
    let (error, partial, dispatch) = failure.into_parts();
    let error = match error.api_error() {
        Some(api)
            if evidence.credentials().reflects(api.message().as_bytes())
                || evidence.credentials().reflects(api.code().as_bytes()) =>
        {
            Error::from_api(crate::ApiError::new(
                api.operation(),
                api.status(),
                crate::error::SafeText::new("", &[]),
                crate::error::SafeText::new("registry response withheld", &[]),
                api.retryable(),
            ))
            .with_cause(error)
        }
        _ => error,
    };
    Failure::new(error, partial, dispatch)
}

fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}

fn before_write<T, U>(failure: Failure<T>) -> Failure<U> {
    not_sent(failure.into_parts().0)
}

fn invalid_input<T>(message: &str) -> Failure<T> {
    not_sent(
        Error::new(ErrorClass::InvalidInput)
            .with_detail(ErrorDetail::InvalidInput)
            .with_cause(registry_error("", None, "", message)),
    )
}

fn write_operation(
    id: &'static str,
    method: Method,
    template: &'static str,
    success: &'static [StatusCode],
) -> Operation {
    Operation {
        method,
        class: Classification::Write,
        retry: Retry::Once,
        success,
        allow_recovery: false,
        no_redirect: true,
        ..read_operation(id, template)
    }
}

fn status(error: &Error) -> Option<u16> {
    error.api_error().and_then(crate::ApiError::status)
}

fn is_server_error(error: &Error) -> bool {
    status(error).is_some_and(|status| (500..600).contains(&status))
}

fn create_failure<T>(failure: Failure<T>, message: &'static str) -> Failure<T> {
    let (error, partial, dispatch) = failure.into_parts();
    if dispatch != WriteDispatch::MayHaveBeenSent
        || status(&error)
            .is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 499))
    {
        return Failure::new(error, partial, dispatch);
    }
    let api = error.api_error();
    let message = if let Some(api) = api.filter(|api| !api.message().is_empty()) {
        format!("{message}: {}", api.message())
    } else {
        message.to_owned()
    };
    let error = registry_error(
        api.map_or("", crate::ApiError::operation),
        status(&error),
        api.map_or("", crate::ApiError::code),
        &message,
    )
    .with_cause(error);
    Failure::new(error, partial, dispatch)
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
            serde_json::to_value(output).unwrap()["Items"][0]["ID"],
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
            serde_json::to_value(output).unwrap()["Items"][0]["ID"],
            "repo-1"
        );
    }
}

#[cfg(test)]
mod write_tests;
