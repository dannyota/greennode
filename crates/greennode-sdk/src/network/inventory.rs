use http::{Method, StatusCode};
use serde_json::value::RawValue;

use crate::endpoint::Base;
use crate::error::SafeText;
use crate::model::wire::{WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::{ApiError, Error, ErrorClass, RequestContext};

use super::Client;

const CAP: usize = 8 << 20;

#[derive(Clone, Copy)]
pub(super) enum InventoryStyle {
    Plain,
    Region,
    Server,
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
            InventoryStyle::Server => api(
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
        crate::wire::strict_json::validate(body.as_bytes(), &[], CAP)
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
            keep_status: protected,
            overflow_overrides_status: protected,
            sensitive: protected,
            reject_reflected_credentials: false,
            no_redirect: protected,
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
        let response = operation::execute_raw(
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
        .await
        .map_err(|error| screen(error.into_parts().0, evidence))?;
        let body = response::classify(
            read.id,
            response.status,
            bytes::Bytes::copy_from_slice(&response.body),
            operation.success,
            (true, protected, operation.retry),
            evidence.credentials(),
        )
        .map_err(|error| screen(*error, evidence))?;
        if protected {
            let text = go_json(&body);
            if operation::reflects_json(text.as_bytes(), evidence.credentials(), true)
                != Some(false)
                || !serde_json::from_str::<serde_json::Value>(&text)
                    .is_ok_and(|value| !evidence.reflected_in(&value))
            {
                return Err(read.shape(evidence));
            }
        }
        Ok(response.reply.body)
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
    error.screen_api(&|text| evidence.text_reflects(text))
}

pub(super) fn value_reflects_id(evidence: &Evidence, value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => evidence.id_reflects(text),
        serde_json::Value::Number(number) => {
            evidence.id_reflects(&number.to_string())
                || number
                    .as_f64()
                    .filter(|value| value.fract() == 0.0 && value.abs() < 1e15)
                    .is_some_and(|value| evidence.id_reflects(&format!("{value:.0}")))
        }
        serde_json::Value::Array(items) => {
            items.iter().any(|item| value_reflects_id(evidence, item))
        }
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, item)| evidence.id_reflects(key) || value_reflects_id(evidence, item)),
        serde_json::Value::Null | serde_json::Value::Bool(_) => false,
    }
}
