//! Read account information from the GreenNode portal.

mod envelope;
mod project_reads;
pub use project_reads::*;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use crate::config::Session;
use crate::wire::product::{
    operation::{self, Operation},
    redirect::{Classification, Retry},
    retry,
};
use crate::{Config, Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use envelope::{Envelope, withheld};

/// Client for portal operations.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
}

/// Selects the current portal user without filters.
#[derive(Clone, Debug, Default)]
pub struct GetUserInfoInput {}

/// Information about the current portal user.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetUserInfoOutput {
    /// Account data with the keys returned by the API.
    #[serde(rename = "UserInfo")]
    pub user_info: UserInfo,
}

/// Portal user details, including account data and API-defined fields.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UserInfo {
    /// Account fields keyed by their API names.
    pub fields: BTreeMap<String, Value>,
}

impl std::fmt::Debug for UserInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UserInfo([REDACTED])")
    }
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
        }
    }

    /// Reads the current user's account information from the portal endpoint.
    pub async fn get_user_info(
        &self,
        ctx: &RequestContext,
        _input: GetUserInfoInput,
    ) -> Result<GetUserInfoOutput, Failure<GetUserInfoOutput>> {
        let failure = |error| Failure::new(error, None, WriteDispatch::NotAWrite);
        let session = &self.session;
        let base = session
            .endpoints()
            .portal
            .as_ref()
            .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let operation = user_info_operation();
        let deadline = session.clock().monotonic_now() + Duration::from_secs(30);
        let ctx = ctx.child_with_deadline(deadline);
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(&ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        operation::execute::<Envelope>(
            runtime,
            base,
            &operation,
            &[],
            &[],
            session.bearer_source(),
            bytes::Bytes::new(),
        )
        .await
        .map(|(_, envelope)| GetUserInfoOutput {
            user_info: envelope.0,
        })
        .map_err(|error| failure(withheld(&operation, error.into_parts().0)))
    }
}

fn user_info_operation() -> Operation {
    Operation {
        id: "portal.GetUserInfo",
        template: "v1/users/info",
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
