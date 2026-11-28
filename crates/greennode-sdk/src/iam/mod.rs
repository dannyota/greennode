//! IAM caller, user, service-account, policy, and group operations.

mod accounts;
mod attachments;
mod groups;
mod models;
mod policies;
mod policy_guard;
mod policy_writes;
mod principal_guard;
mod service_account_writes;
mod user_guard;
mod user_mfa;
mod user_password;
mod user_writes;
mod wire;
mod writes;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use tokio::sync::Mutex;

use http::{Method, StatusCode};

use crate::config::Session;
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation},
    options::Options,
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Config, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use wire::{Body, Screen, Wire};

pub use accounts::*;
pub use attachments::*;
pub use groups::*;
pub use models::*;
pub use policies::*;
pub use policy_writes::*;
pub use service_account_writes::*;
pub use user_mfa::*;
pub use user_writes::*;
pub use writes::WriteError;

const OBJECT_CAP: usize = 256 << 10;
const LIST_CAP: usize = 4 << 20;
const DEFAULT_CAP: usize = 8 << 20;

/// Cause of a failed IAM success-body decode.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The body could not be read as one JSON value.
    MalformedJson,
    /// The JSON did not have the response's expected shape.
    InvalidShape,
}

crate::error::debug_record!(DecodeError; MalformedJson, InvalidShape);

/// IAM client sharing its configuration's authentication session.
#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    guard: Arc<GuardCache>,
}

#[derive(Default)]
struct GuardCache {
    caller: Mutex<Option<GetCallerIdentityOutput>>,
    write_actions: Mutex<Vec<String>>,
}

#[derive(Clone, Copy)]
enum Api {
    Accounts,
    Policies,
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
impl Client {
    /// Creates an IAM client without signing in or sending a request.
    pub fn new(config: &Config) -> Self {
        Self {
            session: config.session().clone(),
            guard: Arc::default(),
        }
    }

    async fn fetch<T: Wire, O>(
        &self,
        ctx: &RequestContext,
        api: Api,
        operation: &Operation,
        ids: &[(&str, &str)],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<T, Failure<O>> {
        let path = validate_ids(operation.id, ids)?;
        let session = &self.session;
        let endpoints = session.endpoints();
        let base = match api {
            Api::Accounts => endpoints.dashboard.as_ref(),
            Api::Policies => endpoints.iam.as_ref(),
        }
        .ok_or_else(|| failure(Error::new(ErrorClass::InvalidConfig)))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|error| failure(*error))?;
        if let Some(configured) = runtime.configured_credentials {
            evidence.absorb(configured);
        }
        if path.iter().any(|id| evidence.id_reflects(id))
            || query.iter().any(|(_, value)| evidence.text_reflects(value))
        {
            return Err(refusal(
                ErrorClass::InvalidInput,
                "InvalidUsage",
                "vngcloud: invalid input: IAM request operand withheld",
            ));
        }
        let (_, Body(output)) = operation::execute_with_evidence::<Body<T>>(
            runtime,
            base,
            operation,
            &Options::DEFAULT,
            &path,
            query,
            session.bearer_source(),
            bytes::Bytes::new(),
            evidence,
        )
        .await
        .map_err(|error| {
            let error = wire::guarded_decode_error(operation, error.into_parts().0);
            let error = wire::account_error(operation.id, error);
            let error = wire::private_status_error(operation, error, evidence);
            failure(screen_error(error, evidence))
        })?;
        if matches!(
            evidence.field_names(),
            crate::wire::strict_json::FieldNames::Canonical
        ) {
            evidence
                .keep_decoded_operands(operation.id, &wire::WireOperands(&output))
                .map_err(|error| failure(*error))?;
        }
        Ok(output)
    }

    async fn read<T: Wire + Screen + serde::Serialize, O>(
        &self,
        ctx: &RequestContext,
        api: Api,
        operation: &Operation,
        ids: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<T, Failure<O>> {
        let mut evidence = Evidence::new();
        let mut output: T = self
            .fetch(ctx, api, operation, ids, query, &mut evidence)
            .await?;
        output.screen(&evidence);
        screened_output(output, &evidence).map_err(|error| failure(*error))
    }
}

fn screened_output<T: serde::Serialize>(output: T, evidence: &Evidence) -> Result<T, Box<Error>> {
    if evidence.serialized_output_reflects(&output) {
        return Err(Box::new(evidence.screen_error(Error::new(ErrorClass::Api))));
    }
    Ok(output)
}

fn screen_error(mut error: Error, evidence: &Evidence) -> Error {
    let reflects_status = |status| evidence.output_reflects(&serde_json::Value::from(status));
    error.remove_details::<crate::HttpStatus>(|status| reflects_status(status.as_u16()));
    let Some(api) = error.api_error() else {
        return error;
    };
    let reflects = |text: &str| evidence.text_reflects(text);
    if !api.status().is_some_and(reflects_status) {
        return error.screen_api(&reflects);
    }
    let message = if evidence.text_reflects(api.message()) {
        "IAM response withheld"
    } else {
        api.message()
    };
    let detail = ApiError::without_fallback(
        api.operation(),
        api.observed_status(),
        SafeText::new(api.code(), &[]),
        SafeText::new(message, &[]),
        api.retryable(),
    )
    .with_public_status_withheld();
    Error::new(error.class())
        .with_cause(Error::from_api(detail))
        .screen_api(&reflects)
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
fn validate_ids<'a, T>(
    operation: &str,
    ids: &[(&str, &'a str)],
) -> Result<Vec<&'a str>, Failure<T>> {
    for (field, value) in ids {
        if value.is_empty() {
            return Err(invalid_input(format!("{operation} requires {field}")));
        }
    }
    for (field, value) in ids {
        if !Options::DEFAULT.path_ids.allows(value) {
            let message = if operation == "iam.GetPolicy" {
                "iam.GetPolicy requires a valid PolicyID".to_owned()
            } else {
                format!("{operation} requires {field} to match ^[A-Za-z0-9-]+$")
            };
            return Err(invalid_input(message));
        }
    }
    Ok(ids.iter().map(|(_, value)| *value).collect())
}

#[expect(clippy::result_large_err, reason = "The SDK returns Failure by value")]
fn require_identity<T>(
    actual: &str,
    expected: &str,
    message: &'static str,
) -> Result<(), Failure<T>> {
    if Options::DEFAULT.path_ids.allows(actual) && actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    Err(refusal(ErrorClass::Api, "", message))
}

fn invalid_input<T>(message: String) -> Failure<T> {
    refusal(
        ErrorClass::InvalidInput,
        "InvalidUsage",
        &format!("vngcloud: invalid input: {message}"),
    )
}

fn refusal<T>(class: ErrorClass, code: &str, message: &str) -> Failure<T> {
    let error = Error::from_api(ApiError::without_fallback(
        "",
        None,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ));
    failure(if class == ErrorClass::Api {
        error
    } else {
        Error::new(class).with_cause(error)
    })
}

fn failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

fn read_operation(
    id: &'static str,
    template: &'static str,
    cap: usize,
    allow_empty: bool,
) -> Operation {
    Operation {
        id,
        template,
        method: Method::GET,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: &[StatusCode::OK],
        cap,
        allow_recovery: true,
        allow_empty,
        no_output: false,
        keep_status: false,
        raw_body: false,
        overflow_overrides_status: false,
        sensitive: false,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: false,
        report_redirect: false,
        retry_config: retry::Config::default(),
    }
}

struct PageQuery {
    page: String,
    size: String,
}

impl PageQuery {
    fn new(page: i64, size: i64) -> Self {
        Self {
            page: page.max(0).to_string(),
            size: if size <= 0 { 10_000 } else { size }.to_string(),
        }
    }

    fn pairs<'a>(&'a self, name: &'a str) -> Vec<(&'static str, &'a str)> {
        let mut pairs = vec![
            ("pageNumber", self.page.as_str()),
            ("pageSize", self.size.as_str()),
        ];
        if !name.is_empty() {
            pairs.push(("name", name));
        }
        pairs
    }
}

macro_rules! id_input {
    ($(#[$meta:meta])* $name:ident, $(#[$field_meta:meta])* $field:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name {
            $(#[$field_meta])*
            pub $field: String,
        }

        crate::error::debug_record!($name { $field });

        impl $name {
            /// Sets the required identifier.
            pub fn new(id: impl Into<String>) -> Self {
                Self { $field: id.into() }
            }
        }
    };
}

macro_rules! page_input {
    ($(#[$meta:meta])* $name:ident, $(#[$field_meta:meta])* $field:ident, filtered) => {
        $(#[$meta])*
        #[derive(Clone, Default, PartialEq, Eq)]
        pub struct $name {
            $(#[$field_meta])*
            pub $field: String,
            /// Zero-based page number.
            pub page: i64,
            /// Page size. Nonpositive values use the service default.
            pub size: i64,
        }

        crate::error::debug_record!($name { $field, page, size });
    };
    ($(#[$meta:meta])* $name:ident, $(#[$field_meta:meta])* $field:ident, required) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name {
            $(#[$field_meta])*
            pub $field: String,
            /// Zero-based page number.
            pub page: i64,
            /// Page size. Nonpositive values use the service default.
            pub size: i64,
        }

        crate::error::debug_record!($name { $field, page, size });

        impl $name {
            /// Sets the required identifier and uses the default page.
            pub fn new(id: impl Into<String>) -> Self {
                Self { $field: id.into(), page: 0, size: 0 }
            }
        }
    };
}

use {id_input, page_input};
