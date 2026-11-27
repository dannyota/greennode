use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Client, ErrorDetail};
use crate::error::SafeText;
use crate::wire::product::{
    evidence::Evidence,
    operation::{GuardOperation, Operation, RouteAuth, execute_raw},
    options::Options,
    redirect::{Classification, Retry},
    response::{classify, decision_text, sanitize_error_text},
    retry,
};
use crate::wire::strict_json::{Shape, ShapeField, validate_shape};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

const RESPONSE_CAP: usize = 8 << 20;
const OK: &[StatusCode] = &[
    StatusCode::OK,
    StatusCode::CREATED,
    StatusCode::ACCEPTED,
    StatusCode::NO_CONTENT,
];

#[derive(Clone, Copy)]
pub(super) struct Call {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: &'static Method,
    pub(super) sensitive: bool,
    pub(super) not_found: Option<&'static str>,
    pub(super) write: bool,
    pub(super) retry: Retry,
}

impl Call {
    pub(super) const fn get(id: &'static str, route: &'static str) -> Self {
        Self {
            id,
            route,
            method: &Method::GET,
            sensitive: false,
            not_found: None,
            write: false,
            retry: Retry::Repeatable,
        }
    }

    pub(super) const fn post(id: &'static str, route: &'static str) -> Self {
        Self {
            method: &Method::POST,
            ..Self::get(id, route)
        }
    }

    pub(super) const fn write(
        id: &'static str,
        route: &'static str,
        method: &'static Method,
    ) -> Self {
        Self {
            method,
            write: true,
            retry: Retry::Once,
            ..Self::get(id, route)
        }
    }

    fn dispatch(&self, sent: bool) -> WriteDispatch {
        match (self.write, sent) {
            (false, _) => WriteDispatch::NotAWrite,
            (true, false) => WriteDispatch::NotSent,
            (true, true) => WriteDispatch::MayHaveBeenSent,
        }
    }

    pub(super) fn unexpected(&self, status: u16) -> Error {
        api_error(
            self.id,
            status,
            "EmptyResponse",
            &format!("vCDN {} returned an unexpected response", self.short()),
            ErrorClass::Api,
            false,
        )
    }

    fn key_error(&self, status: u16, error: &Error, message: &str) -> Error {
        let code = error.api_error().map_or("", ApiError::code);
        api_error(self.id, status, code, message, error_class(status), false)
    }

    fn short(&self) -> &str {
        self.id.strip_prefix("cdn.").unwrap_or(self.id)
    }

    fn refused(&self, status: u16, envelope: GuardEnvelope<'_>, evidence: &Evidence) -> Error {
        let GuardEnvelope(envelope) = envelope;
        if let Some(not_found) = self.not_found
            && empty_data(envelope.data.as_deref())
        {
            return api_error(
                self.id,
                status,
                "NotFound",
                &format!("vCDN {}: {not_found}", self.short()),
                ErrorClass::NotFound,
                false,
            );
        }
        let parsed = envelope.code.as_deref().and_then(parse_code);
        let effective = parsed
            .as_deref()
            .filter(|code| {
                decision_text(code, evidence.credentials())
                    .is_some_and(|safe| safe.as_str() == *code)
            })
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or_default();
        let code = if envelope.code.is_some() && parsed.is_none() {
            String::new()
        } else {
            parsed
                .filter(|code| !code.is_empty())
                .unwrap_or_else(|| "EnvelopeError".to_owned())
        };
        let message = envelope.message.as_deref().unwrap_or_default();
        let decision = decision_text(message, evidence.credentials());
        let decision = decision.as_ref().map_or("", |text| text.as_str());
        let (code, class, detail) = if decision.contains("is not allow to update or delete") {
            (
                code.as_str(),
                ErrorClass::Api,
                Some(ErrorDetail::ResourceBusy),
            )
        } else if decision.starts_with("Not found cdn") {
            ("NotFound", ErrorClass::NotFound, None)
        } else if effective == 202 && decision.starts_with("Last CDN flush cache time") {
            (
                code.as_str(),
                ErrorClass::Api,
                Some(ErrorDetail::PurgeCooldown),
            )
        } else {
            (
                code.as_str(),
                if effective == 202 {
                    ErrorClass::InvalidInput
                } else {
                    error_class(effective)
                },
                None,
            )
        };
        let safe_code = sanitize_error_text(code, evidence.credentials());
        let safe_message = sanitize_error_text(message, evidence.credentials());
        let no_reason = || format!("vCDN {} failed; the server gave no reason", self.short());
        let safe_message = if let Some(fixed) = key_message(effective) {
            fixed.to_owned()
        } else if message.contains(['@', '\u{ff20}', '\u{fe6b}']) {
            format!(
                "vCDN {} refused: the server message named an account user and was withheld; check that every domain is a CDN of this account",
                self.short()
            )
        } else if safe_message.as_str().trim().is_empty() {
            no_reason()
        } else {
            limited(safe_message.as_str()).to_owned()
        };
        let error = api_error(
            self.id,
            status,
            safe_code.as_str(),
            &safe_message,
            class,
            false,
        );
        match detail {
            Some(detail) => error.with_outer_detail(detail),
            None => error,
        }
    }
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    code: Option<Box<RawValue>>,
    message: Option<String>,
    data: Option<Box<RawValue>>,
}

struct GuardEnvelope<'a>(&'a Envelope);

impl<'a> GuardEnvelope<'a> {
    fn checked(
        envelope: &'a Envelope,
        reflection: crate::wire::product::response::Screen,
    ) -> Option<Self> {
        reflection.is_clean().then_some(Self(envelope))
    }
}

pub(super) struct GuardCall(pub(super) Call);

const ENVELOPE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "success",
        shape: Shape::Any,
    },
    ShapeField {
        name: "code",
        shape: Shape::Any,
    },
    ShapeField {
        name: "message",
        shape: Shape::Any,
    },
    ShapeField {
        name: "data",
        shape: Shape::Any,
    },
]);

pub(super) struct Answer {
    pub(super) data: Option<Box<RawValue>>,
    pub(super) evidence: Evidence,
}

impl Client {
    pub(super) async fn call(
        &self,
        ctx: &RequestContext,
        call: &Call,
        path: &[&str],
        body: Bytes,
    ) -> Result<Answer, Failure<()>> {
        let mut evidence = Evidence::new();
        let data = self
            .call_with_evidence(ctx, call, path, body, &mut evidence)
            .await?;
        Ok(Answer { data, evidence })
    }

    pub(super) async fn call_with_evidence(
        &self,
        ctx: &RequestContext,
        call: &Call,
        path: &[&str],
        body: Bytes,
        evidence: &mut Evidence,
    ) -> Result<Option<Box<RawValue>>, Failure<()>> {
        self.call_with_policy(ctx, call, path, body, evidence, false)
            .await
    }

    pub(super) async fn call_guard(
        &self,
        ctx: &RequestContext,
        guard: &GuardCall,
        path: &[&str],
        evidence: &mut Evidence,
    ) -> Result<Option<Box<RawValue>>, Failure<()>> {
        self.call_with_policy(ctx, &guard.0, path, Bytes::new(), evidence, true)
            .await
    }

    async fn call_with_policy(
        &self,
        ctx: &RequestContext,
        call: &Call,
        path: &[&str],
        body: Bytes,
        evidence: &mut Evidence,
        guard: bool,
    ) -> Result<Option<Box<RawValue>>, Failure<()>> {
        let mut operation = operation(
            call.id,
            call.route,
            (*call.method).clone(),
            RESPONSE_CAP,
            call.sensitive,
        );
        if call.write {
            operation.class = Classification::Write;
        }
        operation.retry = call.retry;
        let sent_failure = |error| Failure::new(error, None, call.dispatch(true));
        let guarded;
        let operation = if guard {
            guarded = GuardOperation::new(operation);
            &*guarded
        } else {
            &operation
        };
        let reply = raw(self, ctx, operation, path, body, evidence, false)
            .await
            .map_err(|failure| {
                let dispatch = match failure.write_dispatch() {
                    WriteDispatch::NotAWrite => call.dispatch(false),
                    dispatch => dispatch,
                };
                let (error, _, _) = failure.into_parts();
                if error
                    .detail::<crate::wire::product::operation::RefusedGuard>()
                    .is_some()
                {
                    return Failure::new(error, None, dispatch);
                }
                let status = error
                    .http_status()
                    .map(|status| status.as_u16())
                    .or_else(|| error.api_error().and_then(ApiError::status));
                let error = match status {
                    Some(status) if let Some(message) = key_message(status) => {
                        call.key_error(status, &error, message)
                    }
                    Some(status) if (200..300).contains(&status) => call.unexpected(status),
                    Some(400) if error.class() == ErrorClass::Api => {
                        Error::new(ErrorClass::InvalidInput).with_cause(error)
                    }
                    _ if error.has_class(ErrorClass::NoCredentials) => {
                        error.with_outer_detail(ErrorDetail::MissingApiKey)
                    }
                    _ => error,
                };
                Failure::new(error, None, dispatch)
            })?;
        let status = reply.status.as_u16();
        if !OK.contains(&reply.status) {
            if reply.status.is_success() {
                return Err(sent_failure(call.unexpected(status)));
            }
            let fixed_key_message = key_message(status);
            let error = classify(
                call.id,
                reply.status,
                Bytes::from(reply.reply.body),
                OK,
                (
                    true,
                    call.sensitive && fixed_key_message.is_none(),
                    operation.retry,
                ),
                evidence.credentials(),
            )
            .err()
            .map_or_else(|| call.unexpected(status), |error| *error);
            let error = if let Some(message) = fixed_key_message {
                call.key_error(status, &error, message)
            } else if status == 400 {
                Error::new(ErrorClass::InvalidInput).with_cause(error)
            } else {
                error
            };
            return Err(sent_failure(error));
        }
        let screen = if call.write {
            let screen = evidence.transport_only().go_peer_body_screen(&reply.body);
            screen.clean().ok_or_else(|| {
                sent_failure(crate::wire::product::operation::unsafe_guard_response(
                    call.id,
                ))
            })?;
            screen
        } else {
            reply.reflection
        };
        let names = if guard {
            crate::wire::strict_json::FieldNames::Canonical
        } else {
            evidence.field_names()
        };
        if names.has_case_variant(&reply.body, &ENVELOPE) {
            return Err(sent_failure(
                crate::wire::product::operation::unsafe_guard_response(call.id),
            ));
        }
        names
            .validate(&reply.body, &ENVELOPE, RESPONSE_CAP)
            .map_err(|_| sent_failure(call.unexpected(status)))?;
        let envelope: Envelope = serde_json::from_slice(&reply.body)
            .map_err(|_| sent_failure(call.unexpected(status)))?;
        if !envelope.success {
            let checked = GuardEnvelope::checked(&envelope, screen)
                .ok_or_else(|| sent_failure(call.unexpected(status)))?;
            return Err(sent_failure(call.refused(status, checked, evidence)));
        }
        Ok(envelope.data)
    }
}

pub(super) fn operation(
    id: &'static str,
    template: &'static str,
    method: Method,
    cap: usize,
    sensitive: bool,
) -> Operation {
    Operation {
        id,
        template,
        method,
        cap,
        sensitive,
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: OK,
        allow_recovery: false,
        allow_empty: false,
        no_output: false,
        keep_status: true,
        overflow_overrides_status: false,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect: false,
        retry_config: retry::Config::default(),
    }
}

pub(super) async fn raw(
    client: &Client,
    ctx: &RequestContext,
    operation: &Operation,
    path: &[&str],
    body: Bytes,
    evidence: &mut Evidence,
    public: bool,
) -> Result<crate::wire::product::sender::Response, Failure<()>> {
    let (base, auth) = if public {
        (&client.session.endpoints().cdn_docs, RouteAuth::PublicHtml)
    } else {
        (
            &client.session.endpoints().cdn,
            RouteAuth::CdnKey(client.session.cdn_key()),
        )
    };
    let base = base
        .as_ref()
        .ok_or_else(|| read_failure(Error::new(ErrorClass::InvalidConfig)))?;
    let mut jitter = || client.session.retry_jitter();
    let runtime = client
        .session
        .runtime(ctx, &mut jitter)
        .map_err(|error| read_failure(*error))?;
    execute_raw(
        runtime,
        base,
        operation,
        &Options::DEFAULT,
        path,
        &[],
        auth,
        body,
        evidence,
    )
    .await
}

pub(super) fn convert<T>(failure: Failure<()>) -> Failure<T> {
    let (error, _, dispatch) = failure.into_parts();
    Failure::new(error, None, dispatch)
}

pub(super) fn read_failure<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotAWrite)
}

pub(super) fn empty_data(data: Option<&RawValue>) -> bool {
    data.is_none_or(|data| matches!(data.get().trim(), "null" | "\"\""))
}

pub(super) fn list<T: serde::de::DeserializeOwned>(
    call: &Call,
    data: Option<&RawValue>,
    allow_null: bool,
) -> Result<Vec<T>, Failure<()>> {
    match data.map(RawValue::get) {
        None | Some("null") if allow_null => Ok(Vec::new()),
        Some(text) => {
            let malformed = || read_failure(call.unexpected(200));
            validate_shape(
                text.as_bytes(),
                &Shape::Array(&Shape::Object(&[])),
                RESPONSE_CAP,
            )
            .map_err(|_| malformed())?;
            serde_json::from_str(text).map_err(|_| malformed())
        }
        None => Err(read_failure(call.unexpected(200))),
    }
}

fn parse_code(raw: &RawValue) -> Option<String> {
    if raw.get().starts_with('"') {
        return serde_json::from_str::<String>(raw.get()).ok();
    }
    serde_json::from_str::<serde_json::Number>(raw.get())
        .ok()
        .map(|number| number.to_string())
}

fn key_message(status: u16) -> Option<&'static str> {
    match status {
        401 => Some("vCDN API key rejected: check the key and its expiry"),
        403 => Some("vCDN API key not allowed to call this API"),
        _ => None,
    }
}

fn error_class(status: u16) -> ErrorClass {
    match status {
        400 => ErrorClass::InvalidInput,
        401 => ErrorClass::Auth,
        403 => ErrorClass::PermissionDenied,
        404 => ErrorClass::NotFound,
        _ => ErrorClass::Api,
    }
}

pub(super) fn api_error(
    operation: &'static str,
    status: u16,
    code: &str,
    message: &str,
    class: ErrorClass,
    retryable: bool,
) -> Error {
    let api = Error::from_api(ApiError::new(
        operation,
        Some(status),
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ));
    if class == ErrorClass::Api {
        api
    } else {
        Error::new(class).with_cause(api)
    }
}

fn limited(text: &str) -> &str {
    &text[..text.floor_char_boundary(256)]
}
