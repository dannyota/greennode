use super::reads::{RepositoryRead, valid_id};
use super::*;
use bytes::Bytes;
use serde_json::{Value, json};
use std::time::Duration;

const CREATE: &str = "containerregistry.CreateRepository";
const DELETE: &str = "containerregistry.DeleteRepository";
const RECONCILE: &str =
    "list-repositories --name <name> and match the exact name before creating again";
const CREATE_WAIT: &str = "containerregistry.CreateRepository: repository was not confirmed by a read; the repository exists and this create must not be repeated";
const DELETE_WAIT: &str = "containerregistry.DeleteRepository: repository did not become unreadable; delete was sent and a rerun is safe";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_BOUND: Duration = Duration::from_secs(60);

/// Creates a private repository with a positive quota in gigabytes.
#[derive(Clone, Debug)]
pub struct CreateRepositoryInput {
    /// Repository name, validated by the service.
    pub name: String,
    /// Positive storage quota in gigabytes.
    pub quota_limit_gb: i64,
    /// Returns the accepted repository without waiting for visibility.
    pub no_wait: bool,
}

impl CreateRepositoryInput {
    /// Sets the required fields and enables the confirmation wait.
    pub fn new(name: impl Into<String>, quota_limit_gb: i64) -> Self {
        Self {
            name: name.into(),
            quota_limit_gb,
            no_wait: false,
        }
    }
}

/// The created repository, available as partial output if confirmation fails.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateRepositoryOutput {
    /// Repository with reflected strings replaced and reflected numeric fields zeroed.
    #[serde(skip)]
    pub repository: Repository,
    #[serde(rename = "Repository", serialize_with = "serialize_repository")]
    projection: Value,
}

fn serialize_repository<S: serde::Serializer>(
    value: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;

    let fields = [
        "ID",
        "Name",
        "BackendName",
        "AccessLevel",
        "RegistryURL",
        "QuotaLimitGB",
        "QuotaUsed",
        "ImageCount",
        "AttachedUsers",
        "CreatedAt",
    ];
    let mut output = serializer.serialize_struct("Repository", fields.len())?;
    for key in fields {
        if key == "QuotaUsed"
            && let Some(number) = value[key].as_f64()
        {
            output.serialize_field(key, &number)?;
        } else {
            output.serialize_field(key, &value[key])?;
        }
    }
    output.end()
}

impl CreateRepositoryOutput {
    fn new(mut repository: Repository, evidence: &Evidence) -> Self {
        let mut projection = json!(&repository);
        let quota_reflected =
            crate::output_number::reflects(repository.quota_used, evidence.credentials(), true);
        evidence.redact(&mut projection);
        if quota_reflected {
            projection["QuotaUsed"] = json!("<redacted>");
        }
        for (key, field) in [
            ("ID", &mut repository.id),
            ("Name", &mut repository.name),
            ("BackendName", &mut repository.backend_name),
            ("AccessLevel", &mut repository.access_level),
            ("RegistryURL", &mut repository.registry_url),
            ("CreatedAt", &mut repository.created_at),
        ] {
            if projection.get(key).and_then(Value::as_str) != Some(field.as_str()) {
                *field = "<redacted>".into();
            }
        }
        for (key, field) in [
            ("QuotaLimitGB", &mut repository.quota_limit_gb),
            ("ImageCount", &mut repository.image_count),
            ("AttachedUsers", &mut repository.attached_users),
        ] {
            if projection.get(key).and_then(Value::as_i64) != Some(*field) {
                *field = 0;
            }
        }
        if projection.get("QuotaUsed").and_then(Value::as_f64) != Some(repository.quota_used) {
            repository.quota_used = 0.0;
        }
        Self {
            repository,
            projection,
        }
    }
}

/// Identifies a repository to delete after checking it holds no images.
#[derive(Clone, Debug)]
pub struct DeleteRepositoryInput {
    /// Repository identifier using ASCII letters, digits, and hyphens.
    pub repository_id: String,
    /// Skips the absence wait after the delete is accepted.
    pub no_wait: bool,
}

impl DeleteRepositoryInput {
    /// Selects the repository and enables the absence wait.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            repository_id: id.into(),
            no_wait: false,
        }
    }
}

/// An accepted repository deletion with no resource fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeleteRepositoryOutput {}

#[derive(Default, Deserialize)]
#[serde(transparent)]
struct RepositoryWrite(Repository);

impl Output for RepositoryWrite {
    const SCHEMA: Shape<'static> = REPOSITORY;
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a private repository once, then confirms it is readable within 60 seconds.
    /// A failed confirmation can return the accepted repository as partial output.
    pub async fn create_repository(
        &self,
        ctx: &RequestContext,
        input: CreateRepositoryInput,
    ) -> Result<CreateRepositoryOutput, Failure<CreateRepositoryOutput>> {
        if input.name.is_empty() {
            return Err(invalid_input(
                "containerregistry.CreateRepository requires Name",
            ));
        }
        if input.quota_limit_gb < 1 {
            return Err(invalid_input(
                "containerregistry.CreateRepository requires QuotaLimitGB to be at least 1",
            ));
        }
        let body =
            json!({"repoName": input.name, "quotaLimit": input.quota_limit_gb, "isPublic": false});
        let mut evidence = Evidence::new();
        let RepositoryWrite(repo) = self.request(
            ctx, &Operation { allow_empty: true, reject_reflected_credentials: false, ..write_operation(
                CREATE, Method::POST, "v1/repository", &[StatusCode::ACCEPTED],
            ) }, &[], &[], Bytes::from(body.to_string()), &mut evidence,
        ).await.map_err(|failure| create_failure(failure, concat!(
            "containerregistry.CreateRepository: create may have already reached the server; ",
            "list-repositories --name <name> and match the exact name before creating again",
        )))?;
        if !valid_id(&repo.id) || evidence.credentials().reflects(repo.id.as_bytes()) {
            let message = if repo.id.is_empty() {
                format!("create response had no id; {RECONCILE}")
            } else {
                format!("create response had no usable id; {RECONCILE}")
            };
            return Err(Failure::new(
                registry_error(CREATE, Some(202), "", &message),
                None,
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        if input.no_wait {
            return Ok(CreateRepositoryOutput::new(repo, &evidence));
        }
        match self
            .wait_repository(ctx, &repo.id, false, &mut evidence)
            .await
        {
            Ok(Some(read)) => Ok(CreateRepositoryOutput::new(read, &evidence)),
            result => {
                let output = CreateRepositoryOutput::new(repo, &evidence);
                Err(settlement_failure(
                    result.err(),
                    output,
                    CREATE_WAIT,
                    &evidence,
                ))
            }
        }
    }

    /// Reads the repository and sends one delete only after it reports zero images.
    /// Unless skipped, waits up to 60 seconds for the repository to become unreadable.
    pub async fn delete_repository(
        &self,
        ctx: &RequestContext,
        input: DeleteRepositoryInput,
    ) -> Result<DeleteRepositoryOutput, Failure<DeleteRepositoryOutput>> {
        let id = &input.repository_id;
        if !valid_id(id) {
            return Err(invalid_input(
                "containerregistry.DeleteRepository requires a valid RepositoryID",
            ));
        }
        let mut evidence = Evidence::new();
        let current = self
            .get_repository_evidenced::<()>(ctx, id, &mut evidence)
            .await
            .map_err(before_write)?;
        match (current.image_count_known, current.repository.image_count) {
            (true, 0) => (),
            (true, count) if count > 0 => return Err(not_sent(registry_error(
                "", None, "", &format!("containerregistry: repository holds images: {DELETE}: repository {id} has {count} image(s); delete them first"),
            ).with_detail(ErrorDetail::RepositoryNotEmpty))),
            _ => return Err(not_sent(registry_error(
                "", None, "", "containerregistry.DeleteRepository: the read did not report a valid image count; refusing to delete without confirming it holds no images",
            ).with_detail(ErrorDetail::ImageCountUnknown))),
        }
        self.request::<Repository, _>(
            ctx,
            &Operation {
                allow_empty: true,
                reject_reflected_credentials: false,
                ..write_operation(
                    DELETE,
                    Method::DELETE,
                    "v1/repository/{}",
                    &[StatusCode::ACCEPTED],
                )
            },
            &[id],
            &[],
            Bytes::new(),
            &mut evidence,
        )
        .await?;
        let output = DeleteRepositoryOutput {};
        if !input.no_wait
            && let Err(error) = self.wait_repository(ctx, id, true, &mut evidence).await
        {
            return Err(settlement_failure(
                Some(error),
                output,
                DELETE_WAIT,
                &evidence,
            ));
        }
        Ok(output)
    }

    async fn wait_repository(
        &self,
        ctx: &RequestContext,
        id: &str,
        absent: bool,
        evidence: &mut Evidence,
    ) -> Result<Option<Repository>, Failure<()>> {
        let clock = self.session.clock();
        let ctx = ctx.child_with_deadline(clock.monotonic_now() + POLL_BOUND);
        loop {
            if let Some(end) = ctx.check(clock) {
                return Err(failure(end.into()));
            }
            let read = self.get_repository_evidenced(&ctx, id, evidence).await;
            if let Some(end) = ctx.check(clock) {
                return Err(failure(end.into()));
            }
            match read {
                Ok(RepositoryRead { repository, .. }) if !absent => return Ok(Some(repository)),
                Err(error) if !error.error().has_class(ErrorClass::NotFound) => return Err(error),
                Err(_) if absent => return Ok(None),
                _ => (),
            }
            ctx.child_with_deadline(clock.monotonic_now() + POLL_INTERVAL)
                .wait(clock)
                .await;
        }
    }
}

fn settlement_failure<T>(
    cause: Option<Failure<()>>,
    output: T,
    message: &str,
    evidence: &Evidence,
) -> Failure<T> {
    let cause = cause.map(|failure| failure.into_parts().0);
    let error =
        Error::new(ErrorClass::NotSettled).with_cause(registry_error("", None, "", message));
    let error = match cause {
        Some(cause) => error.with_cause(cause),
        None => error,
    };
    let (error, allowed, dispatch) = evidence
        .with_partial(
            failure::<()>(error),
            Value::Null,
            WriteDispatch::MayHaveBeenSent,
        )
        .into_parts();
    Failure::new(error, allowed.map(|_| output), dispatch)
}
