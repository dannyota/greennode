use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::Value;

use super::user_writes::{accepted, definite_refusal, uncertain_write, user_write};
use super::writes::{guard_failed, not_sent, settle};
use super::{Api, Client, UserDetail, invalid_input};
use crate::error::SafeText;
use crate::wire::product::{evidence::Evidence, redirect::Retry};
use crate::{ApiError, Error, Failure, RequestContext};

const ENABLE: &str = "iam.EnableUserMFA";
const DISABLE: &str = "iam.DisableUserMFA";

macro_rules! mfa_input {
    ($(#[$meta:meta])* $ty:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        pub struct $ty {
            /// Target IAM user resource ID.
            pub user_id: String,
            /// Factor method. The service supports google.
            pub method: String,
        }

        impl $ty {
            /// Sets the target user and factor method.
            pub fn new(id: impl Into<String>, method: impl Into<String>) -> Self {
                Self { user_id: id.into(), method: method.into() }
            }
        }
    };
}

mfa_input!(
    /// Enables an already verified MFA factor for a user.
    EnableUserMfaInput
);
mfa_input!(
    /// Disables an existing MFA factor for a user.
    DisableUserMfaInput
);

/// The observed outcome of an IAM MFA state change.
#[derive(Clone, Debug, Serialize)]
pub struct UserMfaWriteOutput {
    /// The supported factor method.
    #[serde(rename = "Method")]
    pub method: String,
    /// The last confirmed state, if available.
    #[serde(rename = "Enabled")]
    pub enabled: Option<bool>,
    /// Whether a mutation was accepted with HTTP 204.
    #[serde(rename = "Changed")]
    pub changed: bool,
}

/// The MFA state after an enable operation.
pub type EnableUserMfaOutput = UserMfaWriteOutput;
/// The MFA state after a disable operation.
pub type DisableUserMfaOutput = UserMfaWriteOutput;

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Enables a verified Google factor once and confirms the resulting state.
    pub async fn enable_user_mfa(
        &self,
        ctx: &RequestContext,
        input: EnableUserMfaInput,
    ) -> Result<EnableUserMfaOutput, Failure<EnableUserMfaOutput>> {
        self.change_user_mfa(ctx, ENABLE, &input.user_id, &input.method, true)
            .await
    }

    /// Disables a Google factor once and confirms the resulting state.
    pub async fn disable_user_mfa(
        &self,
        ctx: &RequestContext,
        input: DisableUserMfaInput,
    ) -> Result<DisableUserMfaOutput, Failure<DisableUserMfaOutput>> {
        self.change_user_mfa(ctx, DISABLE, &input.user_id, &input.method, false)
            .await
    }

    async fn change_user_mfa(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        id: &str,
        method: &str,
        enable: bool,
    ) -> Result<UserMfaWriteOutput, Failure<UserMfaWriteOutput>> {
        if method != "google" {
            return Err(guard_failed(invalid_input(format!(
                "{op}: Method must be google"
            ))));
        }
        let mut evidence = self.write_evidence(ctx, op, &[("UserID", id)], &[method])?;
        let target = self.guard_user(ctx, op, id, true, &mut evidence).await?;
        let (verified, active) = google_state(&target).ok_or_else(|| unavailable(op))?;
        let output = |enabled, changed, evidence: &Evidence| UserMfaWriteOutput {
            method: if evidence.text_reflects(method) {
                "<redacted>"
            } else {
                method
            }
            .into(),
            enabled,
            changed,
        };
        if enable && !verified || evidence.text_reflects(method) {
            return Err(unavailable(op));
        }
        if active == enable {
            return Ok(output(Some(active), false, &evidence));
        }
        let route = if enable {
            "accounts-api/v1/iam-users/{}/activate-google"
        } else {
            "accounts-api/v1/iam-users/{}/deactivate-google"
        };
        let operation = user_write(
            op,
            Method::POST,
            route,
            Retry::Once,
            &[StatusCode::NO_CONTENT],
        );
        let response_failure = match self
            .write::<Value, _>(ctx, Api::Accounts, &operation, &[id], None, &mut evidence)
            .await
        {
            Ok(_) => None,
            Err(failure) if definite_refusal(&failure) => return Err(failure),
            Err(failure) if accepted(&failure) => Some(failure),
            Err(failure) => return Err(uncertain_write(failure, op, None, &evidence)),
        };
        let confirmed = self
            .fetch_verified_user(ctx, op, id, &mut evidence)
            .await
            .and_then(|user| google_state(&user).ok_or_else(|| unavailable(op)));
        let state = confirmed.as_ref().ok().map(|(_, enabled)| *enabled);
        let confirmation_failure = match confirmed {
            Ok((verified, active))
                if active == enable && (!enable || verified) && !evidence.text_reflects(method) =>
            {
                None
            }
            other => Some(other.err().unwrap_or_else(|| unavailable(op))),
        };
        let partial = output(state, true, &evidence);
        match (response_failure, confirmation_failure) {
            (None, None) => Ok(partial),
            (Some(response), None) | (None, Some(response)) => {
                Err(settle(response, Some(partial), op, &evidence))
            }
            (Some(response), Some(confirmation)) => {
                let (error, _, dispatch) = response.into_parts();
                let combined = Failure::new(
                    error.with_cause(confirmation.into_parts().0),
                    None,
                    dispatch,
                );
                Err(settle(combined, Some(partial), op, &evidence))
            }
        }
    }
}

fn google_state(user: &UserDetail) -> Option<(bool, bool)> {
    let google = user.mfa.as_ref()?.google_authenticator.as_ref()?;
    Some((google.verified?, google.enabled?))
}

fn unavailable(op: &'static str) -> Failure<UserMfaWriteOutput> {
    not_sent(Error::from_api(ApiError::without_fallback(
        op,
        None,
        SafeText::new("RequestFailed", &[]),
        SafeText::new("MFA state is unavailable or unverified", &[]),
        false,
    )))
}
