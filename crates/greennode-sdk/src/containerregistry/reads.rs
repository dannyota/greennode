use super::*;

/// Filters and selects a page of registry users.
#[derive(Clone, Debug, Default)]
pub struct ListUsersInput {
    /// User name filter.
    pub name: String,
    /// Page number, defaulting to 1 when not positive.
    pub page: i64,
    /// Page size, defaulting to 10000 when not positive.
    pub size: i64,
}
/// A page of registry users.
pub type ListUsersOutput = PagedList<User>;

/// Selects a repository and a page of its users.
#[derive(Clone, Debug, Default)]
pub struct ListRepositoryUsersInput {
    /// Repository identifier using ASCII letters, digits, and hyphens.
    pub repository_id: String,
    /// User name filter.
    pub name: String,
    /// Page number, defaulting to 1 when not positive.
    pub page: i64,
    /// Page size, defaulting to 10000 when not positive.
    pub size: i64,
}
/// A page of users attached to a repository.
pub type ListRepositoryUsersOutput = PagedList<User>;

/// Selects a repository by identifier.
#[derive(Clone, Debug)]
pub struct GetRepositoryInput {
    /// Repository identifier using ASCII letters, digits, and hyphens.
    pub repository_id: String,
}
impl GetRepositoryInput {
    /// Stores the identifier for validation by the read.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            repository_id: id.into(),
        }
    }
}
/// The requested repository.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetRepositoryOutput {
    /// Repository with its matching identifier.
    #[serde(rename = "Repository")]
    pub repository: Repository,
}

/// A registry user and its repository permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    default,
    rename_all(serialize = "PascalCase", deserialize = "camelCase")
)]
pub struct User {
    /// User identifier.
    #[serde(
        rename(deserialize = "uuid", serialize = "ID"),
        deserialize_with = "null_default"
    )]
    pub id: String,
    /// User name.
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Backend name.
    #[serde(deserialize_with = "null_default")]
    pub backend_name: String,
    /// User description.
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    /// Whether the user is disabled.
    #[serde(
        rename(deserialize = "disable", serialize = "Disabled"),
        deserialize_with = "null_default"
    )]
    pub disabled: bool,
    /// Expiry timestamp.
    #[serde(deserialize_with = "null_default")]
    pub expired_at: String,
    /// Creation timestamp.
    #[serde(deserialize_with = "null_default")]
    pub created_at: String,
    /// Number of repositories.
    #[serde(
        rename(deserialize = "numberOfRepo", serialize = "NumberOfRepositories"),
        deserialize_with = "null_default"
    )]
    pub number_of_repositories: i64,
    /// Account identifier, accepted as a number or string.
    #[serde(
        rename(deserialize = "userId", serialize = "UserID"),
        deserialize_with = "user_id"
    )]
    pub user_id: String,
    /// Repository permissions.
    #[serde(
        rename(deserialize = "repoPermissionList", serialize = "Repositories"),
        deserialize_with = "null_default"
    )]
    pub repositories: Vec<RepositoryPermission>,
}
/// Permissions for a particular repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct RepositoryPermission {
    /// Repository identifier.
    #[serde(
        rename(deserialize = "repoId", serialize = "RepositoryID"),
        deserialize_with = "null_default"
    )]
    pub repository_id: String,
    /// Repository name.
    #[serde(
        rename(deserialize = "repoName", serialize = "RepositoryName"),
        deserialize_with = "null_default"
    )]
    pub repository_name: String,
    /// Backend repository name.
    #[serde(
        rename(deserialize = "backendRepoName", serialize = "BackendRepositoryName"),
        deserialize_with = "null_default"
    )]
    pub backend_repository_name: String,
    /// Granted policies.
    #[serde(
        rename(deserialize = "policyDtoList", serialize = "Policies"),
        deserialize_with = "null_default"
    )]
    pub policies: Vec<Permission>,
}
fn user_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let raw = Option::<Box<serde_json::value::RawValue>>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(String::new());
    };
    let value = raw.get();
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(serde::de::Error::custom)
    } else {
        serde_json::from_str::<serde_json::Number>(value).map_err(serde::de::Error::custom)?;
        Ok(value.to_owned())
    }
}
const USER_PERMISSION: Shape<'static> = Shape::Object(&[
    field("repoId"),
    field("repoName"),
    field("backendRepoName"),
    ShapeField {
        name: "policyDtoList",
        shape: Shape::Array(&PERMISSION),
    },
]);
const USER: Shape<'static> = Shape::Object(&[
    field("uuid"),
    field("name"),
    field("backendName"),
    field("description"),
    field("disable"),
    field("expiredAt"),
    field("createdAt"),
    field("numberOfRepo"),
    field("userId"),
    ShapeField {
        name: "repoPermissionList",
        shape: Shape::Array(&USER_PERMISSION),
    },
]);

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Users {
    data: Option<Vec<User>>,
    list_data: Option<Vec<User>>,
    #[serde(deserialize_with = "null_default")]
    page: i64,
    #[serde(deserialize_with = "null_default")]
    page_size: i64,
    total_page: Option<i64>,
    total_item: Option<i64>,
}
impl Output for Users {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        ShapeField {
            name: "data",
            shape: Shape::Array(&USER),
        },
        ShapeField {
            name: "listData",
            shape: Shape::Array(&USER),
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
        let reflected = [
            result.page,
            result.page_size,
            result.total_page.unwrap_or_default(),
            result.total_item.unwrap_or_default(),
        ]
        .into_iter()
        .chain(
            result
                .data
                .iter()
                .chain(result.list_data.iter())
                .flatten()
                .map(|u| u.number_of_repositories),
        )
        .any(|v| {
            response.credentials.reflects(v.to_string().as_bytes())
                || crate::output_number::reflects(v as f64, &response.credentials, false)
        });
        if reflected
            || keys
                .keys()
                .filter(|key| {
                    key.eq_ignore_ascii_case("data") || key.eq_ignore_ascii_case("listData")
                })
                .count()
                != 1
        {
            return Err(Box::new(Error::new(ErrorClass::Api)));
        }
        Ok(result)
    }
}
impl Output for Repository {
    const SCHEMA: Shape<'static> = REPOSITORY;
    const CHECK_RENDERED_NUMBERS: bool = true;
    fn decode_response(op: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        let value: Self = operation::decode_response(op, response)?;
        if [
            value.quota_limit_gb,
            value.image_count,
            value.attached_users,
        ]
        .into_iter()
        .any(|v| {
            response.credentials.reflects(v.to_string().as_bytes())
                || crate::output_number::reflects(v as f64, &response.credentials, false)
        }) || crate::output_number::reflects(value.quota_used, &response.credentials, true)
        {
            return Err(Box::new(Error::new(ErrorClass::Api)));
        }
        Ok(value)
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Lists one page of registry users.
    pub async fn list_users(
        &self,
        ctx: &RequestContext,
        input: ListUsersInput,
    ) -> Result<ListUsersOutput, Failure<ListUsersOutput>> {
        self.users(
            ctx,
            "containerregistry.ListUsers",
            "v1/user",
            &[],
            &input.name,
            (input.page, input.size),
        )
        .await
    }
    /// Lists one page of users attached to a repository.
    pub async fn list_repository_users(
        &self,
        ctx: &RequestContext,
        input: ListRepositoryUsersInput,
    ) -> Result<ListRepositoryUsersOutput, Failure<ListRepositoryUsersOutput>> {
        if !valid_id(&input.repository_id) {
            return Err(failure(Error::new(ErrorClass::InvalidInput).with_detail("vngcloud: invalid input: containerregistry.ListRepositoryUsers requires RepositoryID to match ^[A-Za-z0-9-]+$")));
        }
        self.users(
            ctx,
            "containerregistry.ListRepositoryUsers",
            "v1/repository/{}/user",
            &[&input.repository_id],
            &input.name,
            (input.page, input.size),
        )
        .await
    }
    async fn users(
        &self,
        ctx: &RequestContext,
        id: &'static str,
        template: &'static str,
        path: &[&str],
        name: &str,
        (page, size): (i64, i64),
    ) -> Result<PagedList<User>, Failure<PagedList<User>>> {
        let page = page.max(1).to_string();
        let size = if size <= 0 { 10000 } else { size }.to_string();
        let value: Users = self
            .read_path(
                ctx,
                &read_operation(id, template),
                path,
                &[("name", name), ("page", &page), ("size", &size)],
            )
            .await?;
        let (items, total_page, total_item) = match (
            value.data,
            value.list_data,
            value.total_page,
            value.total_item,
        ) {
            (Some(items), None, Some(pages), Some(total))
            | (None, Some(items), Some(pages), Some(total)) => (items, pages, total),
            _ => return Err(failure(Error::new(ErrorClass::Api))),
        };
        let len = i64::try_from(items.len()).unwrap_or(i64::MAX);
        if value.page < 0
            || value.page_size < 0
            || total_page < 0
            || total_item < 0
            || len > total_item
            || (total_page == 0 && total_item != 0)
            || (total_item == 0 && total_page > 1)
            || (total_page <= 1 && len != total_item)
        {
            return Err(failure(Error::new(ErrorClass::Api)));
        }
        Ok(PagedList {
            items,
            page: value.page,
            page_size: value.page_size,
            total_page,
            total_item,
        })
    }
    /// Reads a repository, confirming absence after an ambiguous server error.
    pub async fn get_repository(
        &self,
        ctx: &RequestContext,
        input: GetRepositoryInput,
    ) -> Result<GetRepositoryOutput, Failure<GetRepositoryOutput>> {
        if !valid_id(&input.repository_id) {
            return Err(failure(Error::new(ErrorClass::InvalidInput).with_detail("vngcloud: invalid input: containerregistry.GetRepository requires RepositoryID to match ^[A-Za-z0-9-]+$")));
        }
        let mut get_operation =
            read_operation("containerregistry.GetRepository", "v1/repository/{}");
        get_operation.overflow_overrides_status = true;
        let result: Result<Repository, Failure<GetRepositoryOutput>> = self
            .read_path(ctx, &get_operation, &[&input.repository_id], &[])
            .await;
        match result {
            Ok(repository) if repository.id == input.repository_id => {
                Ok(GetRepositoryOutput { repository })
            }
            Ok(_) => Err(failure(Error::new(ErrorClass::Api).with_detail(
                "containerregistry.GetRepository: repository identity is unproven",
            ))),
            Err(original) => {
                let server_error = original.error().api_error().is_some_and(|api| {
                    !api.code().is_empty()
                        && api
                            .status()
                            .is_some_and(|status| (500..600).contains(&status))
                });
                if !server_error || original.error().is_body_too_large() {
                    return Err(original);
                }
                let mut confirmation = read_operation(
                    "containerregistry.repositoryFoundAfterServerError",
                    "v1/repository",
                );
                confirmation.cap = 4 << 20;
                let result: Result<Repositories, Failure<GetRepositoryOutput>> = self
                    .read(
                        ctx,
                        &confirmation,
                        &[("accessLevel", "ALL"), ("name", ""), ("size", "10000")],
                    )
                    .await;
                if let Ok(list) = result {
                    let items = match (list.data, list.list_data, list.total_page, list.total_item)
                    {
                        (Some(items), None, Some(pages), Some(total))
                        | (None, Some(items), Some(pages), Some(total))
                            if (0..=1).contains(&pages)
                                && total >= 0
                                && i64::try_from(items.len()).ok() == Some(total)
                                && !(pages == 0 && total != 0)
                                && list.page >= 0
                                && list.page_size >= 0 =>
                        {
                            Some(items)
                        }
                        _ => None,
                    };
                    if let Some(items) = items {
                        let mut ids = std::collections::HashSet::new();
                        if items
                            .iter()
                            .all(|item| valid_id(&item.id) && ids.insert(item.id.as_str()))
                            && !ids.contains(input.repository_id.as_str())
                        {
                            return Err(failure(Error::new(ErrorClass::NotFound).with_detail("vngcloud: resource not found: containerregistry.GetRepository: repository absent")));
                        }
                    }
                }
                Err(original)
            }
        }
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
    async fn list_users_uses_default_paging_and_decodes_numeric_user_id() {
        let server = Server::start(vec![response(include_bytes!(
            "../../tests/fixtures/containerregistry/list_users_numeric_userid.json"
        ))]);
        let result = client(&server)
            .list_users(&RequestContext::new().0, ListUsersInput::default())
            .await
            .unwrap();
        assert!(server.heads()[0].starts_with(b"GET /v1/user?name=&page=1&size=10000 "));
        assert_eq!(result.items[0].user_id, "20260101");
    }
    #[tokio::test]
    async fn list_repository_users_sends_repository_and_filters() {
        let server = Server::start(vec![response(include_bytes!(
            "../../tests/fixtures/containerregistry/list_users.json"
        ))]);
        let result = client(&server)
            .list_repository_users(
                &RequestContext::new().0,
                ListRepositoryUsersInput {
                    repository_id: "repo-1".into(),
                    name: "ci".into(),
                    page: 2,
                    size: 10,
                },
            )
            .await
            .unwrap();
        assert!(
            server.heads()[0]
                .starts_with(b"GET /v1/repository/repo-1/user?name=ci&page=2&size=10 ")
        );
        assert_eq!(result.items[0].repositories[0].policies[0].id, "policy-1");
    }
    #[tokio::test]
    async fn oversized_errors_and_confirmation_never_prove_absence() {
        let big_error = vec![b' '; (8 << 20) + 1];
        let big_confirmation = format!(
            "{{\"data\":[],\"totalPage\":0,\"totalItem\":0,\"padding\":\"{}\"}}",
            "x".repeat(4 << 20)
        );
        let error_response = |body: Vec<u8>| Response::Http {
            status: "500 Internal Server Error".into(),
            headers: vec![("Content-Length".into(), body.len().to_string())],
            body,
        };
        let server = Server::start(vec![
            Response::Http {
                status: "500 Internal Server Error".into(),
                headers: vec![("Content-Length".into(), "20".into())],
                body: b"{}".to_vec(),
            },
            response(br#"{"data":[],"totalPage":0,"totalItem":0}"#),
        ]);
        let failure = client(&server)
            .get_repository(&RequestContext::new().0, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert_ne!(failure.error().class(), ErrorClass::NotFound);
        assert_eq!(server.requests(), 1);
        let server = Server::start(vec![error_response(big_error)]);
        let failure = client(&server)
            .get_repository(&RequestContext::new().0, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert_ne!(failure.error().class(), ErrorClass::NotFound);
        assert_eq!(server.requests(), 1);
        let server = Server::start(vec![
            error_response(b"{}".to_vec()),
            response(big_confirmation.as_bytes()),
        ]);
        let failure = client(&server)
            .get_repository(&RequestContext::new().0, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert_ne!(failure.error().class(), ErrorClass::NotFound);
        assert_eq!(server.requests(), 2);
    }

    #[tokio::test]
    async fn null_page_metadata_defaults_to_zero_for_all_reads() {
        let empty = br#"{"data":[],"page":null,"pageSize":null,"totalPage":0,"totalItem":0}"#;
        let server = Server::start(vec![
            response(empty),
            response(empty),
            Response::Http {
                status: "500 Internal Server Error".into(),
                headers: vec![("Content-Length".into(), "2".into())],
                body: b"{}".to_vec(),
            },
            response(empty),
        ]);
        let client = client(&server);
        let ctx = RequestContext::new().0;
        assert_eq!(
            client
                .list_repositories(&ctx, ListRepositoriesInput::default())
                .await
                .unwrap()
                .page,
            0
        );
        assert_eq!(
            client
                .list_users(&ctx, ListUsersInput::default())
                .await
                .unwrap()
                .page_size,
            0
        );
        assert_eq!(
            client
                .get_repository(&ctx, GetRepositoryInput::new("repo-1"))
                .await
                .unwrap_err()
                .error()
                .class(),
            ErrorClass::NotFound
        );
    }

    #[tokio::test]
    async fn repository_failures_keep_distinct_fixed_diagnostics() {
        let server = Server::start(vec![
            response(br#"{"uuid":"other"}"#),
            Response::Http {
                status: "500 Internal Server Error".into(),
                headers: vec![("Content-Length".into(), "2".into())],
                body: b"{}".to_vec(),
            },
            response(br#"{"data":[],"totalPage":0,"totalItem":0}"#),
        ]);
        let client = client(&server);
        let ctx = RequestContext::new().0;
        let invalid = client
            .get_repository(&ctx, GetRepositoryInput::new("../bad"))
            .await
            .unwrap_err();
        assert!(
            invalid
                .error()
                .detail::<&'static str>()
                .unwrap()
                .contains("requires RepositoryID to match")
        );
        let mismatch = client
            .get_repository(&ctx, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert!(
            mismatch
                .error()
                .detail::<&'static str>()
                .unwrap()
                .contains("repository identity is unproven")
        );
        let missing = client
            .get_repository(&ctx, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert!(
            missing
                .error()
                .detail::<&'static str>()
                .unwrap()
                .contains("containerregistry.GetRepository")
        );
    }

    #[tokio::test]
    async fn get_repository_confirms_absence_after_server_error() {
        let server = Server::start(vec![
            Response::Http {
                status: "500 Internal Server Error".into(),
                headers: vec![("Content-Length".into(), "2".into())],
                body: b"{}".to_vec(),
            },
            response(br#"{"data":[],"totalPage":0,"totalItem":0}"#),
        ]);
        let failure = client(&server)
            .get_repository(&RequestContext::new().0, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::NotFound);
        assert!(
            server.heads()[1].starts_with(b"GET /v1/repository?accessLevel=ALL&name=&size=10000 ")
        );
    }

    #[tokio::test]
    async fn invalid_repository_id_fails_without_request() {
        let server = Server::start(vec![]);
        let c = client(&server);
        let ctx = RequestContext::new().0;
        let failure = c
            .get_repository(&ctx, GetRepositoryInput::new("../bad"))
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        let failure = c
            .list_repository_users(
                &ctx,
                ListRepositoryUsersInput {
                    repository_id: "../bad".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(failure.error().class(), ErrorClass::InvalidInput);
        assert_eq!(server.requests(), 0);
    }

    #[tokio::test]
    async fn get_repository_checks_id_and_decodes_fixture() {
        let server = Server::start(vec![response(include_bytes!(
            "../../tests/fixtures/containerregistry/get_repository.json"
        ))]);
        let result = client(&server)
            .get_repository(&RequestContext::new().0, GetRepositoryInput::new("repo-1"))
            .await
            .unwrap();
        assert!(server.heads()[0].starts_with(b"GET /v1/repository/repo-1 "));
        assert_eq!(result.repository.id, "repo-1");
    }
}
