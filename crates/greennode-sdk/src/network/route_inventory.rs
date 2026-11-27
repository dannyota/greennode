use http::{Method, StatusCode};
use serde_json::value::RawValue;

use crate::endpoint::Base;
use crate::error::SafeText;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    response, retry,
};
use crate::wire::strict_json;
use crate::{ApiError, Error, ErrorClass, PagedList, RequestContext, model};

use super::Client;
use super::wire::Fields;

#[cfg(test)]
mod tests;

pub(super) const CAP: usize = 8 << 20;
const ROUTE_TABLE_WITHHELD: &str = "security group response withheld";
const VPN_WITHHELD: &str = "VPN request failed; server details withheld";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseKind {
    Standard,
    RouteTable,
    Vpn,
}

#[derive(Clone, Copy)]
pub(super) struct InventoryRead {
    id: &'static str,
    template: &'static str,
    kind: ResponseKind,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl InventoryRead {
    pub(super) const fn new(id: &'static str, template: &'static str, kind: ResponseKind) -> Self {
        Self { id, template, kind }
    }

    pub(super) const fn standard(id: &'static str, template: &'static str) -> Self {
        Self::new(id, template, ResponseKind::Standard)
    }

    pub(super) fn input_error(self, reason: &str, evidence: &Evidence) -> Error {
        self.validation_error(ErrorClass::InvalidInput, "InvalidUsage", reason, evidence)
    }

    pub(super) fn config_error(self, reason: &str, evidence: &Evidence) -> Error {
        self.validation_error(ErrorClass::InvalidConfig, "InvalidConfig", reason, evidence)
    }

    fn validation_error(
        self,
        class: ErrorClass,
        code: &str,
        reason: &str,
        evidence: &Evidence,
    ) -> Error {
        let message = match class {
            ErrorClass::InvalidConfig => "vngcloud: invalid config",
            _ => "vngcloud: invalid input",
        };
        let api = ApiError::without_fallback(
            "",
            None,
            SafeText::new(code, &[]),
            SafeText::new(&format!("{message}: {} {reason}", self.id), &[]),
            false,
        );
        Error::new(class)
            .with_cause(Error::from_api(api).screen_api(&|text| evidence.text_reflects(text)))
    }

    pub(super) fn check_id(
        self,
        field: &str,
        id: &str,
        charset: PathIds,
        evidence: &Evidence,
    ) -> Result<(), Error> {
        if evidence.id_reflects(id) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        if id.is_empty() {
            return Err(self.input_error(&format!("requires {field}"), evidence));
        }
        if !charset.allows(id) {
            let pattern = match charset {
                PathIds::Strict => "^[A-Za-z0-9-]+$",
                PathIds::WithUnderscore => "^[A-Za-z0-9_-]+$",
            };
            return Err(self.input_error(&format!("requires {field} to match {pattern}"), evidence));
        }
        Ok(())
    }

    pub(super) fn invalid(self, evidence: &Evidence) -> Error {
        match self.kind {
            ResponseKind::Vpn => self.api(
                Some(StatusCode::OK),
                "InvalidResponse",
                "invalid VPN response; body withheld",
                false,
                evidence,
            ),
            ResponseKind::RouteTable => self.api(
                None,
                "InvalidResponse",
                "invalid floating IP response; body withheld",
                false,
                evidence,
            ),
            ResponseKind::Standard => self.api(
                None,
                "",
                &format!("{}: request failed: response failed to decode", self.id),
                false,
                evidence,
            ),
        }
    }

    pub(super) fn incomplete(self, evidence: &Evidence) -> Error {
        self.api(
            None,
            "InvalidResponse",
            "invalid or incomplete inventory; body withheld",
            false,
            evidence,
        )
    }

    fn malformed(self, evidence: &Evidence) -> Error {
        if self.kind == ResponseKind::RouteTable {
            self.api(
                None,
                "",
                "response failed to decode; body withheld",
                false,
                evidence,
            )
        } else {
            self.invalid(evidence)
        }
    }

    pub(super) fn vpn_refused(
        self,
        status: Option<StatusCode>,
        retryable: bool,
        evidence: &Evidence,
    ) -> Error {
        self.api(status, "", VPN_WITHHELD, retryable, evidence)
    }

    fn api(
        self,
        status: Option<StatusCode>,
        code: &str,
        message: &str,
        retryable: bool,
        evidence: &Evidence,
    ) -> Error {
        Error::from_api(ApiError::new(
            self.id,
            status.map(|status| status.as_u16()),
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            retryable,
        ))
        .screen_api(&|text| evidence.text_reflects(text))
    }

    pub(super) fn fields<'a>(
        self,
        text: &'a str,
        evidence: &Evidence,
    ) -> Result<Fields<'a>, Error> {
        let parse_error = || {
            if self.kind == ResponseKind::RouteTable {
                self.incomplete(evidence)
            } else {
                self.invalid(evidence)
            }
        };
        let text = match (self.kind, text) {
            (ResponseKind::Standard, "") => "{}",
            (ResponseKind::RouteTable, "") => return Err(self.invalid(evidence)),
            _ => text,
        };
        let raw: &RawValue = serde_json::from_str(text).map_err(|_| self.malformed(evidence))?;
        if raw.get() == "null" && self.kind == ResponseKind::Standard {
            return serde_json::from_str("{}").map_err(|_| parse_error());
        }
        strict_json::validate(text.as_bytes(), &[], text.len())
            .map_err(|_| self.invalid(evidence))?;
        serde_json::from_str(text).map_err(|_| parse_error())
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    pub(super) fn vserver_inventory_base(&self) -> Result<&Base, Error> {
        self.session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))
    }

    pub(super) async fn inventory_project(
        &self,
        ctx: &RequestContext,
        inputs: &[&str],
        evidence: &mut Evidence,
    ) -> Result<String, Error> {
        if inputs.iter().any(|value| evidence.text_reflects(value)) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| *error)?;
        if evidence.id_reflects(&project) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        Ok(project)
    }

    pub(super) async fn read_route_inventory(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: InventoryRead,
        path: &[&str],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        let operation = Operation {
            id: read.id,
            template: read.template,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: CAP,
            allow_recovery: true,
            allow_empty: false,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: false,
            sensitive: read.kind != ResponseKind::Standard,
            reject_reflected_credentials: false,
            no_redirect: read.kind == ResponseKind::RouteTable,
            retry_config: retry::Config::default(),
        };
        let options = Options {
            path_ids: if read.kind != ResponseKind::Vpn {
                PathIds::WithUnderscore
            } else {
                PathIds::Strict
            },
            report_redirect: read.kind == ResponseKind::RouteTable,
            ..Options::DEFAULT
        };
        let response = operation::execute_raw(
            runtime,
            base,
            &operation,
            &options,
            path,
            query,
            RouteAuth::Bearer(self.session.bearer_source()),
            bytes::Bytes::new(),
            evidence,
        )
        .await
        .map_err(|failure| {
            let error = failure.into_parts().0;
            if matches!(
                error.class(),
                ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
            ) {
                return error.screen_api(&|text| evidence.text_reflects(text));
            }
            if error.is_body_too_large() {
                if read.kind != ResponseKind::Standard {
                    return read.invalid(evidence).with_body_too_large();
                }
            } else if read.kind != ResponseKind::Standard
                && let Some(api) = error.api_error()
                && api.operation() == read.id
            {
                let status = api
                    .status()
                    .and_then(|status| StatusCode::from_u16(status).ok());
                if read.kind == ResponseKind::Vpn {
                    if status == Some(StatusCode::OK) {
                        return read.invalid(evidence);
                    }
                    return read.vpn_refused(status, api.retryable(), evidence);
                }
                if status.is_some() {
                    return read.api(
                        status,
                        api.code(),
                        ROUTE_TABLE_WITHHELD,
                        api.retryable(),
                        evidence,
                    );
                }
            }
            error.screen_api(&|text| evidence.text_reflects(text))
        })?;
        if response.status == StatusCode::OK {
            return Ok(response.reply.body);
        }
        let status = response.status;
        let retryable = matches!(status.as_u16(), 429 | 502..=504);
        if read.kind == ResponseKind::Vpn {
            return Err(read.vpn_refused(Some(status), retryable, evidence));
        }
        response::classify(
            read.id,
            status,
            response.reply.body.into(),
            operation.success,
            (true, false, operation.retry),
            evidence.credentials(),
        )
        .map(Vec::from)
        .map_err(|error| {
            if read.kind == ResponseKind::RouteTable
                && let Some(api) = error.api_error()
            {
                return read.api(
                    Some(status),
                    api.code(),
                    ROUTE_TABLE_WITHHELD,
                    api.retryable(),
                    evidence,
                );
            }
            error.screen_api(&|text| evidence.text_reflects(text))
        })
    }

    pub(super) async fn inventory_page<T: WireField>(
        &self,
        ctx: &RequestContext,
        read: InventoryRead,
        project: &str,
        input: (&str, i64, i64),
        evidence: &mut Evidence,
    ) -> Result<(PagedList<T>, usize), Error> {
        let (name, page, size) = input;
        let page = if page > 0 { page } else { model::DEFAULT_PAGE }.to_string();
        let size = if size > 0 {
            size
        } else {
            model::DEFAULT_PAGE_SIZE
        }
        .to_string();
        let query = [
            ("name", name),
            ("page", page.as_str()),
            ("size", size.as_str()),
        ];
        let base = self.vserver_inventory_base()?;
        let raw = self
            .read_route_inventory(ctx, base, read, &[project], &query, evidence)
            .await?;
        let text = go_json(&raw);
        let fields = read.fields(&text, evidence)?;
        let output = (|| {
            Ok(PagedList {
                items: field(&fields, "listData")?,
                page: field(&fields, "page")?,
                page_size: field(&fields, "pageSize")?,
                total_page: field(&fields, "totalPage")?,
                total_item: field(&fields, "totalItem")?,
            })
        })()
        .map_err(|()| read.invalid(evidence))?;
        Ok((output, raw.len()))
    }

    pub(super) async fn list_route_inventory<T: WireField + serde::Serialize>(
        &self,
        ctx: &RequestContext,
        read: InventoryRead,
        input: (&str, i64, i64),
    ) -> Result<PagedList<T>, Error> {
        let mut evidence = self.request_evidence(ctx)?;
        let project = self
            .inventory_project(ctx, &[input.0], &mut evidence)
            .await?;
        let (mut output, _) = self
            .inventory_page(ctx, read, &project, input, &mut evidence)
            .await?;
        redact_page(read, &mut output, &evidence)?;
        Ok(output)
    }
}

pub(super) fn field<T: WireField>(fields: &Fields<'_>, name: &str) -> Result<T, ()> {
    fields
        .output_raw(name)?
        .map(|raw| serde_json::from_str::<Field<T>>(raw.get()).map_err(|_| ()))
        .transpose()
        .map(|field| field.map_or_else(T::default, |field| field.0))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
pub(super) fn redact_page<T: WireField + serde::Serialize>(
    read: InventoryRead,
    output: &mut PagedList<T>,
    evidence: &Evidence,
) -> Result<(), Error> {
    let numbers = [
        output.page,
        output.page_size,
        output.total_page,
        output.total_item,
    ];
    if numbers
        .into_iter()
        .any(|mut value| evidence.text_reflects(&value.to_string()) || !value.redact(evidence))
        || ids_reflect(&output.items, evidence)
        || !output.items.redact(evidence)
    {
        return Err(read.invalid(evidence));
    }
    Ok(())
}

pub(super) fn ids_reflect<T: serde::Serialize>(value: &T, evidence: &Evidence) -> bool {
    fn scan(value: &serde_json::Value, evidence: &Evidence, numeric: bool) -> bool {
        match value {
            serde_json::Value::Number(number) if numeric => {
                evidence.text_reflects(&number.to_string())
                    || number.as_f64().is_none_or(|value| {
                        [
                            crate::output_number::Style::Json,
                            crate::output_number::Style::Fixed,
                        ]
                        .into_iter()
                        .any(|style| {
                            crate::output_number::f64(value, style)
                                .is_none_or(|rendered| evidence.text_reflects(&rendered))
                        })
                    })
            }
            serde_json::Value::Array(items) => {
                items.iter().any(|item| scan(item, evidence, numeric))
            }
            serde_json::Value::Object(fields) => fields.iter().any(|(name, value)| {
                ((name.ends_with("ID") || name.ends_with("UUID"))
                    && value.as_str().is_some_and(|id| evidence.id_reflects(id)))
                    || scan(
                        value,
                        evidence,
                        numeric || name == "TunnelLimit" || name == "CircuitID",
                    )
            }),
            _ => false,
        }
    }
    serde_json::to_value(value).map_or(true, |value| scan(&value, evidence, false))
}

macro_rules! paged_input {
    ($(#[$meta:meta])* $input:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default)]
        pub struct $input {
            /// Optional display-name filter.
            pub name: String,
            /// Page number. Nonpositive values use the default.
            pub page: i64,
            /// Page size. Nonpositive values use the default.
            pub size: i64,
        }
    };
}

macro_rules! required_id_input {
    ($(#[$meta:meta])* $input:ident, $id:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        pub struct $input {
            /// Identifies the resource in the selected project.
            pub $id: String,
        }

        impl $input {
            /// Selects the resource by identifier.
            pub fn new(id: impl Into<String>) -> Self {
                Self { $id: id.into() }
            }
        }
    };
}

pub(super) use {paged_input, required_id_input};
