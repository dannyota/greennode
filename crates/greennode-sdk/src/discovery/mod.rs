use crate::wire::product::{
    DiscoveryDocument,
    evidence::Evidence,
    operation::{self, Operation},
    options::{Options, Pacing},
    redirect::{Classification, Retry},
    retry, screened_discovery_stream,
    sender::{BearerSource, Runtime, SentCredentials},
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Clock, Error, ErrorClass, Project, RequestContext};
use http::{Method, StatusCode};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    task::Poll,
};
use tokio::sync::{Mutex, MutexGuard};

pub(crate) enum ProjectRead {
    Id,
    Metadata,
}

pub(crate) trait InventoryLimits: Send {
    fn reserve(&mut self) -> Result<(), Box<Error>>;
    fn check_bytes(&self, bytes: usize) -> Result<(), Box<Error>>;
    fn account(&mut self, rows: usize, bytes: usize) -> Result<(), Box<Error>>;
    fn pacing(&self) -> &Pacing;
}

#[derive(Default)]
pub(crate) struct Discovery {
    selected: Mutex<Option<Project>>,
    #[cfg(test)]
    before_publish: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Discovery {
    pub(crate) fn new(project_id: Option<String>, region: &str) -> Self {
        let selected = project_id.filter(|id| !id.is_empty()).map(|id| Project {
            id,
            region: region.to_owned(),
            ..Default::default()
        });
        Self {
            selected: Mutex::new(selected),
            #[cfg(test)]
            before_publish: None,
        }
    }

    async fn lock(
        &self,
        ctx: &RequestContext,
        clock: &dyn Clock,
    ) -> Result<MutexGuard<'_, Option<Project>>, Box<Error>> {
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        let mut lock = Box::pin(self.selected.lock());
        let mut ended = Box::pin(ctx.wait(clock));
        let state = std::future::poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(Box::new(end.into())));
            }
            lock.as_mut().poll(cx).map(Ok)
        })
        .await?;
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        Ok(state)
    }

    pub(crate) async fn project_id_context(
        &self,
        ctx: &RequestContext,
        clock: &dyn Clock,
    ) -> Result<Option<String>, Box<Error>> {
        let state = self.lock(ctx, clock).await?;
        Ok(state.as_ref().map(|project| project.id.clone()))
    }

    pub(crate) async fn require(
        &self,
        runtime: Runtime<'_>,
        base: &crate::endpoint::Base,
        source: &dyn BearerSource,
        region: &str,
        read: ProjectRead,
    ) -> Result<Project, Box<Error>> {
        self.require_with_evidence(runtime, base, source, region, read, &mut Evidence::new())
            .await
    }

    pub(crate) async fn require_with_evidence(
        &self,
        runtime: Runtime<'_>,
        base: &crate::endpoint::Base,
        source: &dyn BearerSource,
        region: &str,
        read: ProjectRead,
        evidence: &mut Evidence,
    ) -> Result<Project, Box<Error>> {
        self.require_with_limits(runtime, base, source, region, read, evidence, None)
            .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "A project read uses the existing operation inputs and optional inventory limits"
    )]
    pub(crate) async fn require_with_limits(
        &self,
        runtime: Runtime<'_>,
        base: &crate::endpoint::Base,
        source: &dyn BearerSource,
        region: &str,
        read: ProjectRead,
        evidence: &mut Evidence,
        mut limits: Option<&mut dyn InventoryLimits>,
    ) -> Result<Project, Box<Error>> {
        let ctx = runtime.ctx.child_with_deadline(runtime.deadline);
        let clock = runtime.clock;
        let mut state = self.lock(&ctx, clock).await?;
        if matches!(read, ProjectRead::Id)
            && let Some(project) = &*state
        {
            return Ok(project.clone());
        }
        if let Some(limits) = limits.as_deref_mut() {
            limits.reserve()?;
        }
        let projects =
            read_projects_with_evidence(runtime, base, source, region, evidence, limits).await?;
        let selected = select(projects, region, state.as_ref())?;
        if evidence.id_reflects(&selected.id) {
            return Err(invalid_response(StatusCode::OK));
        }
        #[cfg(test)]
        if let Some(hook) = &self.before_publish {
            hook();
        }
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        *state = Some(selected.clone());
        Ok(selected)
    }
}

pub(crate) async fn read_projects(
    runtime: Runtime<'_>,
    base: &crate::endpoint::Base,
    source: &dyn BearerSource,
    region: &str,
) -> Result<Vec<Project>, Box<Error>> {
    read_projects_with_evidence(runtime, base, source, region, &mut Evidence::new(), None).await
}

async fn read_projects_with_evidence(
    runtime: Runtime<'_>,
    base: &crate::endpoint::Base,
    source: &dyn BearerSource,
    region: &str,
    evidence: &mut Evidence,
    limits: Option<&mut dyn InventoryLimits>,
) -> Result<Vec<Project>, Box<Error>> {
    let op = projects_operation();
    let ctx = runtime.ctx;
    let clock = runtime.clock;
    let pacing = limits.as_deref().map(InventoryLimits::pacing);
    let (status, document) = operation::execute_with_evidence::<DiscoveryDocument>(
        runtime,
        base,
        &op,
        &Options {
            pacing,
            project_discovery: true,
            response_guard: pacing.map(|_| guard_estimate_project_response as _),
            ..Options::DEFAULT
        },
        &[],
        &[],
        source,
        bytes::Bytes::new(),
        evidence,
    )
    .await
    .map_err(|failure| Box::new(failure.into_parts().0))?;
    if let Some(limits) = limits.as_deref() {
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        limits.check_bytes(document.raw_len)?;
    }
    let raw = document.raw.ok_or_else(|| invalid_response(status))?;
    if evidence.guards_reads()
        && crate::wire::strict_json::FieldNames::Canonical
            .has_case_variant(raw.get().as_bytes(), &Inventory::SCHEMA)
    {
        return Err(Box::new(
            operation::unsafe_guard_response(LIST_PROJECTS).with_http_status(status),
        ));
    }
    crate::wire::strict_json::validate_shape(
        raw.get().as_bytes(),
        &Inventory::SCHEMA,
        raw.get().len(),
    )
    .map_err(|_| invalid_response(status))?;
    let inventory: Inventory =
        serde_json::from_str(raw.get()).map_err(|_| invalid_response(status))?;
    let mut ids = HashSet::new();
    let mut projects = inventory
        .projects
        .ok_or_else(|| invalid_response(StatusCode::OK))?;
    if inventory.extra.keys().any(|key| {
        matches!(
            key.as_str(),
            "page"
                | "pageSize"
                | "paging"
                | "pagination"
                | "totalPage"
                | "totalPages"
                | "next"
                | "nextPage"
                | "isNext"
                | "hasMore"
                | "continuationToken"
        )
    }) || projects
        .iter()
        .any(|project| project.id.is_empty() || !ids.insert(&project.id))
    {
        return Err(invalid_response(StatusCode::OK));
    }
    let rows = projects.len();
    projects.retain_mut(|project| {
        if project.region.is_empty() {
            project.region = region.to_owned();
        }
        project.region == region
    });
    let credentials = &document.credentials;
    if (!projects.is_empty() && credentials.reflects(region.as_bytes()))
        || projects
            .iter()
            .any(|project| credentials.integer_output_reflects(project.user_id))
    {
        return Err(invalid_response(StatusCode::OK));
    }
    if let Some(limits) = limits {
        limits.account(rows, document.raw_len)?;
    }
    Ok(projects)
}

const LIST_PROJECTS: &str = "project.ListProjects";
const PROJECTS_OK: &[StatusCode] = &[StatusCode::OK];

fn invalid_response(status: StatusCode) -> Box<Error> {
    crate::wire::product::response::project_discovery_error(LIST_PROJECTS, status)
}

fn select(
    projects: Vec<Project>,
    region: &str,
    selected: Option<&Project>,
) -> Result<Project, Box<Error>> {
    let mut matches = projects.into_iter();
    if let Some(selected) = selected {
        return Ok(matches
            .find(|project| project.id == selected.id)
            .unwrap_or_else(|| Project {
                id: selected.id.clone(),
                region: region.to_owned(),
                ..Default::default()
            }));
    }
    let project = matches
        .next()
        .ok_or_else(|| Box::new(Error::new(ErrorClass::ProjectNotFound)))?;
    if matches.next().is_some() {
        return Err(Box::new(Error::new(ErrorClass::ProjectAmbiguous)));
    }
    Ok(project)
}

fn projects_operation() -> Operation {
    Operation {
        id: LIST_PROJECTS,
        template: "v1/projects",
        method: Method::GET,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: PROJECTS_OK,
        cap: 8 << 20,
        allow_recovery: true,
        allow_empty: false,
        no_output: false,
        keep_status: false,
        raw_body: false,
        overflow_overrides_status: false,
        sensitive: true,
        reflection: crate::wire::product::operation::Reflection::Refuse,
        no_redirect: true,
        report_redirect: true,
        retry_config: retry::Config::default(),
    }
}

#[derive(Default, serde::Deserialize)]
struct Inventory {
    projects: Option<Vec<Project>>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl Inventory {
    const SCHEMA: Shape<'static> = Shape::Object(&[ShapeField {
        name: "projects",
        shape: Shape::Array(&Shape::Object(&[
            ShapeField {
                name: "projectId",
                shape: Shape::Any,
            },
            ShapeField {
                name: "name",
                shape: Shape::Any,
            },
            ShapeField {
                name: "region",
                shape: Shape::Any,
            },
            ShapeField {
                name: "status",
                shape: Shape::Any,
            },
            ShapeField {
                name: "isDefault",
                shape: Shape::Any,
            },
            ShapeField {
                name: "userId",
                shape: Shape::Any,
            },
        ])),
    }]);
}

#[cfg(test)]
mod tests;

fn guard_estimate_project_response(
    _: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &SentCredentials,
    sent: &SentCredentials,
) -> Option<Box<Error>> {
    screened_discovery_stream(body, known, sent, true)
        .is_none()
        .then(|| invalid_response(status))
}
