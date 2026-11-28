use std::collections::{BTreeMap, HashSet};

use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::value::RawValue;

use crate::error::SafeText;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, RouteAuth},
    options::{Options, PathIds},
    redirect::{Classification, Retry},
    retry,
};
use crate::wire::strict_json::{self, field_matches};
use crate::{
    ApiError, DEFAULT_PAGE, DEFAULT_PAGE_SIZE, Error, ErrorClass, PagedList, RequestContext,
};

use super::super::Client;

const CAP: usize = 8 << 20;
const MAX_PAGES: i64 = 50;
const MAX_ROWS: i64 = 500_000;
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    pub(super) static READ_DEADLINE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
    pub(super) static WALK_DEADLINE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

pub(super) fn read_deadline() -> std::time::Duration {
    #[cfg(test)]
    if let Some(deadline) = READ_DEADLINE.with(std::cell::Cell::get) {
        return deadline;
    }
    DEADLINE
}

pub(super) fn walk_deadline() -> std::time::Duration {
    #[cfg(test)]
    if let Some(deadline) = WALK_DEADLINE.with(std::cell::Cell::get) {
        return deadline;
    }
    DEADLINE
}

const WITHHELD: &str = "security group response withheld";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    Plain,
    Protected,
    Inventory,
}

#[derive(Clone, Copy)]
pub(super) struct Read {
    id: &'static str,
    template: &'static str,
    style: Style,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Read {
    pub(super) const fn new(id: &'static str, template: &'static str, style: Style) -> Self {
        Self {
            id,
            template,
            style,
        }
    }

    pub(super) fn path(self, field: &str, id: &str) -> Result<(), Error> {
        if PathIds::Strict.allows(id) {
            return Ok(());
        }
        let requirement = if id.is_empty() {
            ""
        } else {
            " to match ^[A-Za-z0-9-]+$"
        };
        let message = format!(
            "vngcloud: invalid input: {} requires {field}{requirement}",
            self.id
        );
        Err(
            Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(
                ApiError::without_fallback(
                    "",
                    None,
                    SafeText::new("InvalidUsage", &[]),
                    SafeText::new(&message, &[]),
                    false,
                ),
            )),
        )
    }

    fn api(self, status: Option<u16>, code: &str, message: &str, retryable: bool) -> Error {
        Error::from_api(ApiError::without_fallback(
            self.id,
            status,
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            retryable,
        ))
    }

    fn syntax(self) -> Error {
        if self.style == Style::Plain {
            let message = format!("{}: request failed: response failed to decode", self.id);
            self.api(None, "", &message, false)
        } else {
            self.api(None, "", "response failed to decode; body withheld", false)
        }
    }

    fn protected_invalid(self) -> Error {
        self.api(
            None,
            "InvalidResponse",
            "invalid floating IP response; body withheld",
            false,
        )
    }

    fn invalid(self) -> Error {
        match self.style {
            Style::Plain => self.syntax(),
            Style::Protected => self.protected_invalid(),
            Style::Inventory => self.api(
                None,
                "InvalidResponse",
                "invalid or incomplete inventory; body withheld",
                false,
            ),
        }
    }
}

pub(super) struct Reads<'a> {
    client: &'a Client,
    ctx: &'a RequestContext,
    evidence: Evidence,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl<'a> Reads<'a> {
    pub(super) fn new(client: &'a Client, ctx: &'a RequestContext) -> Result<Self, Error> {
        Ok(Self {
            client,
            ctx,
            evidence: client.request_evidence(ctx)?,
        })
    }

    pub(super) fn screen(&self, error: Error) -> Error {
        self.evidence.screen_error(error)
    }

    pub(super) fn sanitize<T: WireField + serde::Serialize>(
        &self,
        read: Read,
        mut output: T,
    ) -> Result<T, Error> {
        let value = serde_json::to_value(&output).map_err(|_| self.invalid(read))?;
        if output.identifiers_reflect(&self.evidence)
            || self.evidence.output_fields_reflect_with(&value, &|number| {
                self.protected_number(number.as_str())
            })
        {
            return Err(self.invalid(read));
        }
        if output.redact(&self.evidence) {
            Ok(output)
        } else {
            Err(self.invalid(read))
        }
    }

    fn protected_number(&self, text: &str) -> bool {
        let credentials = self.evidence.credentials();
        if credentials.has_numeric_product_credential() {
            return true;
        }
        let hex = data_encoding::HEXLOWER.encode(text.as_bytes());
        credentials
            .values()
            .iter()
            .any(|secret| secret.expose_secret().eq_ignore_ascii_case(&hex))
    }

    pub(super) fn protected_output<T: serde::Serialize>(
        &self,
        read: Read,
        output: &T,
    ) -> Result<(), Error> {
        let value =
            serde_json::to_value(output).map_err(|_| self.screen(read.protected_invalid()))?;
        let text =
            serde_json::to_string(&value).map_err(|_| self.screen(read.protected_invalid()))?;
        if self.evidence.text_reflects(&text)
            || self.evidence.output_reflects(&value)
            || self.evidence.output_fields_reflect_with(&value, &|number| {
                self.protected_number(number.as_str())
            })
        {
            return Err(self.screen(read.protected_invalid()));
        }
        Ok(())
    }

    pub(super) fn append_bounded<T: serde::Serialize>(
        &self,
        read: Read,
        retained: &mut usize,
        items: &mut Vec<T>,
        rows: Vec<T>,
    ) -> Result<(), Error> {
        let byte_limit = self.client.session.retained_byte_limit(CAP);
        for row in rows {
            let length = serde_json::to_vec(&row)
                .map_err(|_| self.invalid(read))?
                .len();
            *retained = retained
                .checked_add(length + 1)
                .ok_or_else(|| self.invalid(read))?;
            if *retained > byte_limit {
                return Err(self.invalid(read));
            }
            items.push(row);
        }
        Ok(())
    }

    pub(super) fn invalid(&self, read: Read) -> Error {
        self.screen(read.invalid())
    }

    pub(super) fn safe_identity(&self, id: &str, path_ids: PathIds) -> bool {
        path_ids.allows(id) && !self.evidence.id_reflects(id)
    }

    async fn request(
        &mut self,
        read: Read,
        ids: &[&str],
        query: &[(&str, &str)],
    ) -> Result<String, Error> {
        let reflects_inputs = |evidence: &Evidence| {
            ids.iter().any(|id| evidence.id_reflects(id))
                || query.iter().any(|(_, value)| evidence.text_reflects(value))
        };
        if reflects_inputs(&self.evidence) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let session = &self.client.session;
        let scoped;
        let ctx = if read.style == Style::Plain {
            self.ctx
        } else {
            scoped = self
                .ctx
                .child_with_deadline(session.clock().monotonic_now() + read_deadline());
            &scoped
        };
        let project = session
            .require_project_id_with_evidence(ctx, &mut self.evidence)
            .await
            .map_err(|err| self.screen(*err))?;
        if self.evidence.id_reflects(&project) || reflects_inputs(&self.evidence) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().copied())
            .collect();
        let base = session
            .endpoints()
            .vserver
            .as_ref()
            .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))?;
        let mut jitter = || session.retry_jitter();
        let runtime = session
            .runtime(ctx, &mut jitter)
            .map_err(|err| self.screen(*err))?;
        let protected = read.style != Style::Plain;
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
            reflection: crate::wire::product::operation::Reflection::Redact,
            no_redirect: protected,
            report_redirect: protected,
            retry_config: retry::Config::default(),
        };
        let reply = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            &path,
            query,
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::new(),
            &mut self.evidence,
        )
        .await
        .map_err(|err| {
            let err = err.into_parts().0;
            if protected {
                if err.is_body_too_large() {
                    return self.screen(read.protected_invalid()).with_body_too_large();
                }
                if let Some(api) = err.api_error()
                    && api.is_for_operation(read.id)
                    && let Some(status) = api.observed_status()
                {
                    return self.screen(
                        read.api(Some(status), api.code(), WITHHELD, api.retryable())
                            .retaining_response_proof(&err),
                    );
                }
            }
            self.screen(err)
        })?;
        if reply.status != StatusCode::OK {
            let Err(error) = reply.classify_normalized_json(
                read.id,
                operation.success,
                (true, false, operation.retry),
            ) else {
                return Err(self.invalid(read));
            };
            let error = if protected && let Some(api) = error.api_error() {
                read.api(api.observed_status(), api.code(), WITHHELD, api.retryable())
                    .retaining_response_proof(&error)
            } else {
                *error
            };
            return Err(self.screen(error));
        }
        let text = reply
            .decode_projection(|body| Ok(go_json(body).into_owned()))
            .map_err(|error| self.screen(*error))?;
        if text.is_empty() {
            return if protected {
                Err(self.screen(read.protected_invalid()))
            } else {
                Ok("null".into())
            };
        }
        let raw =
            serde_json::from_str::<&RawValue>(&text).map_err(|_| self.screen(read.syntax()))?;
        strict_json::validate(raw.get().as_bytes(), &[], CAP).map_err(|_| {
            self.screen(if protected {
                read.protected_invalid()
            } else {
                read.invalid()
            })
        })?;
        if protected && self.evidence.text_reflects(&text) {
            return Err(self.screen(read.protected_invalid()));
        }
        Ok(text)
    }

    pub(super) async fn get<T: WireField>(&mut self, read: Read, ids: &[&str]) -> Result<T, Error> {
        self.decode(read, ids, |raw| {
            serde_json::from_str::<Field<T>>(raw)
                .map(|field| field.0)
                .map_err(|_| ())
        })
        .await
    }

    pub(super) async fn decode<T>(
        &mut self,
        read: Read,
        ids: &[&str],
        parse: impl FnOnce(&str) -> Result<T, ()>,
    ) -> Result<T, Error> {
        let raw = self.request(read, ids, &[]).await?;
        parse(&raw).map_err(|()| self.invalid(read))
    }

    pub(super) async fn page<T: WireField>(
        &mut self,
        read: Read,
        name: &str,
        page: i64,
        size: i64,
    ) -> Result<PagedList<T>, Error> {
        let page = if page > 0 { page } else { DEFAULT_PAGE }.to_string();
        let size = if size > 0 { size } else { DEFAULT_PAGE_SIZE }.to_string();
        let raw = self
            .request(
                read,
                &[],
                &[("name", name), ("page", &page), ("size", &size)],
            )
            .await?;
        parse_page(&raw, read.style == Style::Inventory).map_err(|()| self.invalid(read))
    }

    pub(super) async fn complete<T: WireField>(
        &mut self,
        read: Read,
        invalid_read: Read,
        path_ids: PathIds,
        identity: fn(&T) -> &str,
    ) -> Result<Vec<T>, Error> {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut metadata = None;
        for page in 1..=MAX_PAGES {
            let out = self.page::<T>(read, "", page, DEFAULT_PAGE_SIZE).await?;
            let invalid = || self.invalid(invalid_read);
            let current = (out.total_page, out.total_item, out.page_size);
            let count = i64::try_from(out.items.len()).map_err(|_| invalid())?;
            if out.page != page
                || out.page_size <= 0
                || !(0..=MAX_PAGES).contains(&out.total_page)
                || !(0..=MAX_ROWS).contains(&out.total_item)
                || count > out.page_size
                || count > DEFAULT_PAGE_SIZE
                || (out.total_item > 0 && (out.total_page == 0 || count == 0))
                || *metadata.get_or_insert(current) != current
            {
                return Err(invalid());
            }
            for item in &out.items {
                let id = identity(item);
                if !self.safe_identity(id, path_ids) || !seen.insert(id.to_owned()) {
                    return Err(invalid());
                }
            }
            items.extend(out.items);
            let received = i64::try_from(items.len()).map_err(|_| invalid())?;
            if received > out.total_item {
                return Err(invalid());
            }
            if page >= out.total_page {
                if received != out.total_item
                    || seen.iter().any(|id| !self.safe_identity(id, path_ids))
                {
                    return Err(invalid());
                }
                return Ok(items);
            }
        }
        Err(self.invalid(invalid_read))
    }

    pub(super) fn sanitize_page<T: WireField + serde::Serialize>(
        &self,
        read: Read,
        page: PagedList<T>,
    ) -> Result<PagedList<T>, Error> {
        let mut metadata = [page.page, page.page_size, page.total_page, page.total_item];
        if metadata.iter().any(|value| {
            self.protected_number(&value.to_string())
                || self.evidence.integer_output_reflects(*value)
        }) || !metadata
            .iter_mut()
            .all(|value| value.redact(&self.evidence))
        {
            return Err(self.invalid(read));
        }
        Ok(PagedList {
            items: self.sanitize(read, page.items)?,
            ..page
        })
    }
}

pub(super) fn objects(text: &str) -> Result<BTreeMap<String, &RawValue>, ()> {
    serde_json::from_str::<Option<_>>(text)
        .map(Option::unwrap_or_default)
        .map_err(|_| ())
}

pub(super) fn field<T: WireField>(
    fields: &BTreeMap<String, &RawValue>,
    key: &str,
) -> Result<T, ()> {
    let mut matches = fields.keys().filter(|name| field_matches(name, key));
    if matches.next().is_some() && matches.next().is_some() {
        return Err(());
    }
    fields
        .get(key)
        .map(|raw| {
            serde_json::from_str::<Field<T>>(raw.get())
                .map(|field| field.0)
                .map_err(|_| ())
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_page<T: WireField>(raw: &str, guarded: bool) -> Result<PagedList<T>, ()> {
    let fields = objects(raw)?;
    let list: Option<Vec<T>> = field(&fields, "listData")?;
    let page: Option<i64> = field(&fields, "page")?;
    let size: Option<i64> = field(&fields, "pageSize")?;
    let pages: Option<i64> = field(&fields, "totalPage")?;
    let total: Option<i64> = field(&fields, "totalItem")?;
    if guarded {
        let (Some(items), Some(page), Some(size), Some(pages), Some(total)) =
            (&list, page, size, pages, total)
        else {
            return Err(());
        };
        let count = i64::try_from(items.len()).map_err(|_| ())?;
        if page < 1
            || size < 1
            || pages < 0
            || total < count
            || count > size
            || (count > 0 && pages < page)
            || (total == 0 && pages > 1)
        {
            return Err(());
        }
    }
    Ok(PagedList {
        items: list.unwrap_or_default(),
        page: page.unwrap_or_default(),
        page_size: size.unwrap_or_default(),
        total_page: pages.unwrap_or_default(),
        total_item: total.unwrap_or_default(),
    })
}
