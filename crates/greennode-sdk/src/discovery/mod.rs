use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    redirect::{Classification, Retry},
    retry,
    sender::{BearerSource, Response, Runtime, SentCredentials},
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{ApiError, Clock, Error, ErrorClass, Project, RequestContext};
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
        let ctx = runtime.ctx.child_with_deadline(runtime.deadline);
        let clock = runtime.clock;
        let mut state = self.lock(&ctx, clock).await?;
        if matches!(read, ProjectRead::Id)
            && let Some(project) = &*state
        {
            return Ok(project.clone());
        }
        let projects = read_projects_with_evidence(runtime, base, source, region, evidence).await?;
        let selected = select(projects, region, state.as_ref())?;
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
    read_projects_with_evidence(runtime, base, source, region, &mut Evidence::new()).await
}

async fn read_projects_with_evidence(
    runtime: Runtime<'_>,
    base: &crate::endpoint::Base,
    source: &dyn BearerSource,
    region: &str,
    evidence: &mut Evidence,
) -> Result<Vec<Project>, Box<Error>> {
    let (_, inventory) = operation::execute_with_evidence::<Inventory>(
        runtime,
        base,
        &projects_operation(),
        &Options::DEFAULT,
        &[],
        &[],
        source,
        bytes::Bytes::new(),
        evidence,
    )
    .await
    .map_err(|failure| {
        let (error, _, _) = failure.into_parts();
        if error.class() == ErrorClass::Api
            && error.detail::<StatusCode>() == Some(&StatusCode::OK)
            && !error.is_body_too_large()
        {
            invalid_response(StatusCode::OK)
        } else {
            Box::new(error)
        }
    })?;
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
    projects.retain_mut(|project| {
        if project.region.is_empty() {
            project.region = region.to_owned();
        }
        project.region == region
    });
    let credentials = &inventory.credentials;
    if (!projects.is_empty() && credentials.reflects(region.as_bytes()))
        || projects.iter().any(|project| {
            credentials.reflects(project.user_id.to_string().as_bytes())
                || crate::output_number::reflects(project.user_id as f64, credentials, false)
        })
    {
        return Err(invalid_response(StatusCode::OK));
    }
    Ok(projects)
}

const LIST_PROJECTS: &str = "project.ListProjects";

fn invalid_response(status: StatusCode) -> Box<Error> {
    Box::new(
        Error::from_api(ApiError::without_fallback(
            LIST_PROJECTS,
            Some(status.as_u16()),
            SafeText::new("InvalidResponse", &[]),
            SafeText::new("invalid discovery response; body withheld", &[]),
            false,
        ))
        .with_outer_detail(status),
    )
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

#[derive(Default, serde::Deserialize)]
struct Inventory {
    projects: Option<Vec<Project>>,
    #[serde(skip)]
    credentials: SentCredentials,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl Output for Inventory {
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

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        if !operation.success.contains(&response.status)
            && operation::reflects_json(&response.body, &response.credentials, false)
                .unwrap_or(false)
        {
            return Err(invalid_response(response.status));
        }
        Ok(Self {
            credentials: response.credentials.clone(),
            ..operation::decode_response(operation, response)?
        })
    }
}

#[cfg(test)]
mod tests;
