#![allow(dead_code)]

use crate::wire::product::{
    operation::{self, Operation, Output},
    redirect::{Classification, Retry},
    retry,
    sender::{BearerSource, Runtime},
};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Project};
use http::{Method, StatusCode};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    task::Poll,
};
use tokio::sync::Mutex;

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

    pub(crate) async fn require(
        &self,
        runtime: Runtime<'_>,
        base: &crate::endpoint::Base,
        source: &dyn BearerSource,
        region: &str,
    ) -> Result<Project, Box<Error>> {
        let effective = runtime.ctx.child_with_deadline(runtime.deadline);
        let ctx = &effective;
        let clock = runtime.clock;
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        let mut lock = Box::pin(self.selected.lock());
        let mut ended = Box::pin(ctx.wait(clock));
        let mut state = std::future::poll_fn(|cx| {
            if let Poll::Ready(end) = ended.as_mut().poll(cx) {
                return Poll::Ready(Err(Box::new(end.into())));
            }
            lock.as_mut().poll(cx).map(Ok)
        })
        .await?;
        if let Some(end) = ctx.check(clock) {
            return Err(Box::new(end.into()));
        }
        if let Some(project) = &*state {
            return Ok(project.clone());
        }
        let (_, inventory) = operation::execute::<Inventory>(
            runtime,
            base,
            &projects_operation(),
            &[],
            &[],
            source,
            bytes::Bytes::new(),
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
        let mut ids = HashSet::new();
        let projects = inventory.projects.ok_or_else(invalid_response)?;
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
            return Err(invalid_response());
        }
        let selected = select(projects, region)?;
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

fn invalid_response() -> Box<Error> {
    Box::new(Error::new(ErrorClass::Api).with_outer_detail(StatusCode::OK))
}

fn select(projects: Vec<Project>, region: &str) -> Result<Project, Box<Error>> {
    let mut matches = projects.into_iter().filter_map(|mut project| {
        if project.region.is_empty() {
            project.region = region.to_owned();
        }
        (project.region == region).then_some(project)
    });
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
        id: "project.ListProjects",
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
}

#[cfg(test)]
mod tests;
