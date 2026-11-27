//! List projects visible to the configured account.

#[cfg(test)]
mod tests;

pub use crate::model::Project;

use std::sync::Arc;

use crate::config::Session;
use crate::{Config, Error, ErrorClass, Failure, List, RequestContext, WriteDispatch, discovery};

/// Client for listing projects.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    region: String,
}

/// Filters the project inventory.
#[derive(Clone, Default)]
pub struct ListProjectsInput {
    /// Region to include. An empty value uses the configured region.
    pub region: String,
}

crate::error::debug_record!(ListProjectsInput { region });

/// Projects visible to the configured account in the selected region.
pub type ListProjectsOutput = List<Project>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Builds a client without I/O and shares the configuration's login session.
    pub fn new(config: &Config) -> Self {
        Self {
            session: Arc::clone(config.session()),
            region: config.region().to_owned(),
        }
    }

    /// Reads the regional inventory and filters it without selecting a project.
    pub async fn list_projects(
        &self,
        ctx: &RequestContext,
        input: ListProjectsInput,
    ) -> Result<ListProjectsOutput, Failure<ListProjectsOutput>> {
        crate::wire::product::exit::finish!(async {
            let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
            let base = self
                .session
                .endpoints()
                .vserver
                .as_ref()
                .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
            let region = if input.region.is_empty() {
                &self.region
            } else {
                &input.region
            };
            let mut jitter = || self.session.retry_jitter();
            let runtime = self
                .session
                .runtime(ctx, &mut jitter)
                .map_err(|error| failure(*error))?;
            let items =
                discovery::read_projects(runtime, base, self.session.bearer_source(), region)
                    .await
                    .map_err(|error| failure(*error))?;
            Ok(List { items })
        })
    }
}
