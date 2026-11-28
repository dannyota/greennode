use http::{Method, StatusCode};
use serde_json::value::RawValue;

use crate::endpoint::Base;
use crate::error::SafeText;
use crate::model::wire::WireField;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    retry,
};
use crate::{ApiError, Error, ErrorClass, RequestContext};

use super::Client;

const CAP: usize = 8 << 20;

#[derive(Clone, Copy)]
pub(super) enum InventoryStyle {
    Plain,
    Region,
    Server,
    ServerGuard,
}

#[derive(Clone, Copy)]
pub(super) struct InventoryRead {
    pub(super) id: &'static str,
    pub(super) template: &'static str,
    pub(super) style: InventoryStyle,
}

pub(super) struct Selection<'a> {
    pub(super) path: &'a [&'a str],
    pub(super) query: &'a [(&'a str, &'a str)],
    pub(super) headers: &'a [(&'static str, &'a str)],
}

impl<'a> Selection<'a> {
    pub(super) fn path(path: &'a [&'a str]) -> Self {
        Self {
            path,
            query: &[],
            headers: &[],
        }
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl InventoryRead {
    pub(super) fn check_path(self, field: &str, id: &str) -> Result<(), Error> {
        if PathIds::Strict.allows(id) {
            return Ok(());
        }
        let requirement = if id.is_empty() {
            String::new()
        } else {
            " to match ^[A-Za-z0-9-]+$".into()
        };
        Err(Error::new(ErrorClass::InvalidInput).with_cause(api(
            "",
            "InvalidUsage",
            &format!(
                "vngcloud: invalid input: {} requires {field}{requirement}",
                self.id
            ),
        )))
    }

    pub(super) fn shape(self, evidence: &Evidence) -> Error {
        let error = match self.style {
            InventoryStyle::Plain => self.syntax(),
            InventoryStyle::Region => api(
                self.id,
                "InvalidResponse",
                "invalid or incomplete inventory; body withheld",
            ),
            InventoryStyle::Server | InventoryStyle::ServerGuard => api(
                self.id,
                "InvalidResponse",
                "invalid server response; body withheld",
            ),
        };
        screen(error, evidence)
    }

    pub(super) fn refused(self, evidence: &Evidence) -> Error {
        screen(
            api(
                self.id,
                "InvalidResponse",
                "invalid endpoint response; body withheld",
            ),
            evidence,
        )
    }

    fn syntax(self) -> Error {
        api(self.id, "", "response failed to decode; body withheld")
    }

    pub(super) fn decode<'a, T: serde::Deserialize<'a>>(
        self,
        body: &'a str,
        evidence: &Evidence,
    ) -> Result<T, Error> {
        if serde_json::from_str::<&RawValue>(body).is_err() {
            return Err(screen(self.syntax(), evidence));
        }
        crate::wire::strict_json::validate(body.as_bytes(), &[], body.len())
            .map_err(|_| self.shape(evidence))?;
        serde_json::from_str(body).map_err(|_| self.shape(evidence))
    }

    pub(super) fn retain<T: WireField>(
        self,
        mut output: T,
        evidence: &Evidence,
    ) -> Result<T, Error> {
        if output.redact(evidence) {
            Ok(output)
        } else {
            Err(self.shape(evidence))
        }
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    pub(super) async fn inventory(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: InventoryRead,
        selection: Selection<'_>,
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        let protected = !matches!(read.style, InventoryStyle::Plain);
        let guarded_server = matches!(read.style, InventoryStyle::ServerGuard);
        let operation = Operation {
            id: read.id,
            template: read.template,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: CAP,
            allow_recovery: true,
            allow_empty: !protected,
            no_output: false,
            keep_status: false,
            raw_body: false,
            overflow_overrides_status: false,
            sensitive: protected,
            reflection: if guarded_server {
                operation::Reflection::Guard
            } else {
                operation::Reflection::Redact
            },
            no_redirect: protected,
            report_redirect: protected,
            retry_config: retry::Config::default(),
        };
        let options = Options {
            headers: selection.headers,
            ..Options::DEFAULT
        };
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        let result = operation::execute_raw(
            runtime,
            base,
            &operation,
            &options,
            selection.path,
            selection.query,
            RouteAuth::Bearer(self.session.bearer_source()),
            bytes::Bytes::new(),
            evidence,
        )
        .await;
        let map_error = |error: Error| {
            if guarded_server
                && let Some(api) = error.api_error()
                && api.observed_status().is_some()
            {
                return super::mutations::with_write_message(
                    read.id,
                    error,
                    "server response withheld",
                    evidence,
                );
            }
            screen(error, evidence)
        };
        let response =
            result.map_err(|error: crate::Failure<()>| map_error(error.into_parts().0))?;
        response
            .classify(
                read.id,
                operation.success,
                (true, protected, operation.retry),
                false,
            )
            .map_err(|error| map_error(*error))?;
        if protected && response.clean_receipt().is_none() {
            return Err(read.shape(evidence));
        }
        response
            .decode_projection(|body| Ok(body.to_vec()))
            .map_err(|error| screen(*error, evidence))
    }
}

fn api(id: &'static str, code: &str, message: &str) -> Error {
    Error::from_api(ApiError::without_fallback(
        id,
        None,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
}

pub(super) fn screen(error: Error, evidence: &Evidence) -> Error {
    evidence.screen_error(error)
}
