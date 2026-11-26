use std::collections::BTreeMap;

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
    response, retry,
};
use crate::wire::strict_json::{self, field_matches};
use crate::{ApiError, Error, ErrorClass, PagedList, RequestContext};

use super::Client;

/// Failure decoding or screening a successful regional response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The body was not valid JSON.
    MalformedJson,
    /// The JSON did not match the operation's response shape.
    InvalidShape,
    /// The response reflected a credential in a field that could not be redacted.
    ReflectedCredential,
}

pub(super) struct CompleteNotFound;

pub(super) struct PoolInUseRefusal(pub(super) SafeText);

pub(super) fn pool_in_use_refusal(body: &[u8], evidence: &Evidence) -> Option<PoolInUseRefusal> {
    response::inspect_error_message(body, |message| {
        let phrase = b"is used in listener";
        message
            .as_bytes()
            .windows(phrase.len())
            .any(|part| part.eq_ignore_ascii_case(phrase))
            .then(|| {
                PoolInUseRefusal(response::sanitize_error_text(
                    message.trim(),
                    evidence.credentials(),
                ))
            })
    })
    .flatten()
}

pub(super) struct Read {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
}

impl Read {
    pub(super) fn decode_error(&self, evidence: &Evidence, detail: DecodeError) -> Box<Error> {
        let message = format!("{}: request failed: response failed to decode", self.id);
        Box::new(self.api("", &message, evidence).with_detail(detail))
    }

    pub(super) fn api(&self, code: &str, message: &str, evidence: &Evidence) -> Error {
        Error::from_api(ApiError::without_fallback(
            self.id,
            None,
            SafeText::new(code, &[]),
            SafeText::new(message, &[]),
            false,
        ))
        .screen_api(&|value| evidence.text_reflects(value))
    }

    pub(super) fn decode<T: WireField>(
        &self,
        text: &str,
        evidence: &Evidence,
    ) -> Result<T, Box<Error>> {
        self.retain(self.parse(text, evidence)?, evidence)
    }

    pub(super) fn parse<T: WireField>(
        &self,
        text: &str,
        evidence: &Evidence,
    ) -> Result<T, Box<Error>> {
        serde_json::from_str::<Field<T>>(text)
            .map(|row| row.0)
            .map_err(|_| self.decode_error(evidence, DecodeError::InvalidShape))
    }

    pub(super) fn retain<T: WireField>(
        &self,
        mut value: T,
        evidence: &Evidence,
    ) -> Result<T, Box<Error>> {
        if !value.redact(evidence) {
            return Err(self.decode_error(evidence, DecodeError::ReflectedCredential));
        }
        Ok(value)
    }
}

pub(super) fn validate_ids(
    operation: &str,
    ids: &[(&str, &str)],
    policy: PathIds,
) -> Result<(), Box<Error>> {
    if let Some((field, _)) = ids.iter().find(|(_, value)| value.is_empty()) {
        return Err(invalid_input(&format!("{operation} requires {field}")));
    }
    if let Some((field, _)) = ids.iter().find(|(_, value)| !policy.allows(value)) {
        let pattern = match policy {
            PathIds::Strict => "^[A-Za-z0-9-]+$",
            PathIds::WithUnderscore => "^[A-Za-z0-9_-]+$",
        };
        return Err(invalid_input(&format!(
            "{operation} requires {field} to match {pattern}"
        )));
    }
    Ok(())
}

pub(super) fn invalid_input(message: &str) -> Box<Error> {
    Box::new(
        Error::new(ErrorClass::InvalidInput).with_cause(Error::from_api(
            ApiError::without_fallback(
                "",
                None,
                SafeText::new("InvalidUsage", &[]),
                SafeText::new(&format!("vngcloud: invalid input: {message}"), &[]),
                false,
            ),
        )),
    )
}

pub(super) struct Document {
    text: String,
    pub(super) evidence: Evidence,
}

impl Document {
    pub(super) fn direct<T: WireField>(&self, read: &Read) -> Result<T, Box<Error>> {
        read.decode(&self.text, &self.evidence)
    }

    pub(super) fn object(&self, read: &Read) -> Result<Envelope<'_>, Box<Error>> {
        Envelope::parse(&self.text, read, &self.evidence)
    }
}

pub(super) struct Envelope<'a>(BTreeMap<String, &'a RawValue>);

impl Envelope<'_> {
    pub(super) fn parse<'a>(
        text: &'a str,
        read: &Read,
        evidence: &Evidence,
    ) -> Result<Envelope<'a>, Box<Error>> {
        serde_json::from_str::<Option<_>>(text)
            .map(|fields| Envelope(fields.unwrap_or_default()))
            .map_err(|_| read.decode_error(evidence, DecodeError::InvalidShape))
    }

    pub(super) fn raw(
        &self,
        read: &Read,
        evidence: &Evidence,
        name: &str,
    ) -> Result<&str, Box<Error>> {
        if self
            .0
            .keys()
            .filter(|key| field_matches(name, key))
            .take(2)
            .count()
            > 1
        {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        Ok(self.0.get(name).map_or("null", |raw| raw.get()))
    }

    pub(super) fn canonical_raw(
        &self,
        read: &Read,
        evidence: &Evidence,
        name: &str,
    ) -> Result<&str, Box<Error>> {
        if self
            .0
            .keys()
            .filter(|key| field_matches(name, key))
            .nth(1)
            .is_some()
        {
            return Err(read.decode_error(evidence, DecodeError::InvalidShape));
        }
        Ok(self.0.get(name).map_or("null", |raw| raw.get()))
    }

    pub(super) fn field<T: WireField>(
        &self,
        read: &Read,
        evidence: &Evidence,
        name: &str,
    ) -> Result<T, Box<Error>> {
        read.decode(self.raw(read, evidence, name)?, evidence)
    }

    pub(super) fn children<T: WireField>(
        &self,
        read: &Read,
        evidence: &Evidence,
    ) -> Result<Vec<T>, Box<Error>> {
        let raw = self.raw(read, evidence, "data")?;
        if raw == "null" {
            return Err(Box::new(read.api(
                "",
                "list response had no data",
                evidence,
            )));
        }
        read.decode(raw, evidence)
    }

    pub(super) fn page<T: WireField>(
        &self,
        read: &Read,
        evidence: &Evidence,
    ) -> Result<PagedList<T>, Box<Error>> {
        Ok(PagedList {
            items: self.field(read, evidence, "listData")?,
            page: self.field(read, evidence, "page")?,
            page_size: self.field(read, evidence, "pageSize")?,
            total_page: self.field(read, evidence, "totalPage")?,
            total_item: self.field(read, evidence, "totalItem")?,
        })
    }
}

impl Client {
    pub(super) async fn read(
        &self,
        ctx: &RequestContext,
        read: &Read,
        ids: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<Document, Box<Error>> {
        let mut evidence = Evidence::new();
        let text = self
            .read_text(ctx, read, ids, query, &mut evidence, false)
            .await?;
        Ok(Document { text, evidence })
    }

    pub(super) async fn read_text(
        &self,
        ctx: &RequestContext,
        read: &Read,
        ids: &[(&str, &str)],
        query: &[(&str, &str)],
        evidence: &mut Evidence,
        require_complete: bool,
    ) -> Result<String, Box<Error>> {
        let session = &self.session;
        let mut jitter = || session.retry_jitter();
        let runtime = session.runtime(ctx, &mut jitter)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        screen_input(evidence, ids.iter().chain(query).map(|(_, value)| *value))?;
        validate_ids(read.id, ids, PathIds::Strict)?;
        let project = session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| Box::new(error.screen_api(&|text| evidence.text_reflects(text))))?;
        let path: Vec<_> = std::iter::once(project.as_str())
            .chain(ids.iter().map(|(_, value)| *value))
            .collect();
        screen_input(
            evidence,
            path.iter()
                .copied()
                .chain(query.iter().map(|(_, value)| *value)),
        )?;
        let base = session
            .endpoints()
            .vlb
            .as_ref()
            .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
        let operation = Operation {
            id: read.id,
            template: read.route,
            method: Method::GET,
            class: Classification::Read,
            retry: Retry::Repeatable,
            success: &[StatusCode::OK],
            cap: 8 << 20,
            allow_recovery: true,
            allow_empty: true,
            no_output: false,
            keep_status: false,
            overflow_overrides_status: require_complete,
            sensitive: false,
            reject_reflected_credentials: false,
            no_redirect: require_complete,
            retry_config: retry::Config::default(),
        };
        let result = operation::execute_raw(
            runtime,
            base,
            &operation,
            &Options::DEFAULT,
            &path,
            query,
            RouteAuth::Bearer(session.bearer_source()),
            Bytes::new(),
            evidence,
        )
        .await;
        let screen =
            |error: Error| Box::new(error.screen_api(&|text| evidence.text_reflects(text)));
        let received = result.map_err(|error| screen(error.into_parts().0))?;
        let status = received.status;
        let in_use = (require_complete && !operation.success.contains(&status))
            .then(|| pool_in_use_refusal(&received.reply.body, evidence))
            .flatten();
        let body = response::classify(
            read.id,
            status,
            Bytes::from(received.reply.body),
            operation.success,
            (true, false, operation.retry),
            evidence.credentials(),
        )
        .map_err(|error| {
            let mut error = *screen(*error);
            if require_complete && status == StatusCode::NOT_FOUND {
                error = error.with_outer_detail(CompleteNotFound);
            }
            if let Some(in_use) = in_use {
                error = error.with_outer_detail(in_use);
            }
            Box::new(error)
        })?;
        let text = if body.is_empty() {
            "null".into()
        } else {
            go_json(&body).into_owned()
        };
        serde_json::from_str::<&RawValue>(&text)
            .map_err(|_| read.decode_error(evidence, DecodeError::MalformedJson))?;
        strict_json::validate(text.as_bytes(), &[], text.len())
            .map_err(|_| read.decode_error(evidence, DecodeError::InvalidShape))?;
        Ok(text)
    }
}

pub(super) fn screen_input<'a>(
    evidence: &Evidence,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), Box<Error>> {
    if values.into_iter().any(|text| evidence.text_reflects(text)) {
        return Err(invalid_input("request contains credential material"));
    }
    Ok(())
}
