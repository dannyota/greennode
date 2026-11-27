use bytes::Bytes;
use http::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use super::Client;
use super::wire::{Family, Read, input_error};
use crate::model::wire::go_json;
use crate::wire::product::{
    evidence::Evidence,
    operation::{Operation, RouteAuth, execute_raw},
    options::Options,
    redirect::{Classification, Retry},
    response::classify_full,
    retry,
};
use crate::wire::strict_json::{Shape, validate_canonical_shape};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

/// Monitor write failure identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WriteError {
    /// The channel validation endpoint rejected the supplied OTP.
    OtpRejected,
    /// The current check status does not support a toggle.
    UnexpectedStatus,
    /// The toggle may have acted, but no confirmation reached its target.
    StatusUnconfirmed,
}

pub(super) struct Mutation {
    pub(super) id: &'static str,
    pub(super) route: &'static str,
    pub(super) method: &'static Method,
    pub(super) success: &'static [StatusCode],
    pub(super) limit: usize,
    pub(super) keep_status: bool,
    pub(super) retry: Retry,
    pub(super) no_redirect: bool,
    pub(super) private: bool,
}

impl Mutation {
    pub(super) const fn standard(
        id: &'static str,
        route: &'static str,
        method: &'static Method,
    ) -> Self {
        Self {
            id,
            route,
            method,
            success: &[StatusCode::OK],
            limit: 8 << 20,
            keep_status: false,
            retry: Retry::Once,
            no_redirect: false,
            private: true,
        }
    }

    pub(super) fn error(
        &self,
        status: Option<u16>,
        code: &str,
        message: &str,
        evidence: &Evidence,
    ) -> Error {
        Read {
            id: self.id,
            route: self.route,
            limit: self.limit,
            family: Family::Plain { strict: false },
        }
        .error(status, code, message, false, evidence)
    }

    pub(super) fn invalid(&self, status: u16, evidence: &Evidence) -> Error {
        self.error(
            Some(status),
            "InvalidResponse",
            "response withheld because credential safety could not be confirmed",
            evidence,
        )
    }
}

pub(super) struct Reply {
    pub(super) status: u16,
    body: Vec<u8>,
}

impl Reply {
    pub(super) fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    pub(super) fn decode<T: DeserializeOwned>(
        &self,
        mutation: &Mutation,
        shape: &Shape<'_>,
        evidence: &Evidence,
    ) -> Result<T, Error> {
        let text = if self.body.is_empty() && matches!(shape, Shape::Object(_)) {
            std::borrow::Cow::Borrowed("{}")
        } else {
            go_json(&self.body)
        };
        let invalid = || {
            mutation.error(
                None,
                "",
                "response failed to decode; body withheld",
                evidence,
            )
        };
        let raw: &RawValue = serde_json::from_str(&text).map_err(|_| invalid())?;
        validate_canonical_shape(raw.get().as_bytes(), shape, raw.get().len())
            .map_err(|_| mutation.invalid(self.status, evidence))?;
        let body = if raw.get() == "null" && matches!(shape, Shape::Object(_)) {
            "{}"
        } else {
            raw.get()
        };
        serde_json::from_str(body).map_err(|_| invalid())
    }
}

#[derive(Default)]
pub(super) struct SendGuard<'a> {
    pub(super) ids: &'a [&'a str],
    pub(super) privacy: Option<&'a Evidence>,
    pub(super) dispatch: Option<&'a crate::wire::product::options::DispatchRecord>,
}

impl Client {
    pub(super) fn write_evidence(
        &self,
        ctx: &RequestContext,
        operands: &[&str],
    ) -> Result<Evidence, Error> {
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if operands.iter().any(|value| evidence.text_reflects(value)) {
            return Err(input_error("monitor request operand withheld".into()));
        }
        Ok(evidence)
    }

    pub(super) async fn mutate<T>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        path: &[&str],
        body: Option<&dyn SerializeBody>,
        evidence: &mut Evidence,
    ) -> Result<Reply, Failure<T>> {
        self.mutate_guarded(ctx, mutation, path, body, evidence, SendGuard::default())
            .await
    }

    pub(super) async fn mutate_guarded<T>(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        path: &[&str],
        body: Option<&dyn SerializeBody>,
        evidence: &mut Evidence,
        guard: SendGuard<'_>,
    ) -> Result<Reply, Failure<T>> {
        let base = self
            .session
            .endpoints()
            .monitor
            .as_ref()
            .ok_or_else(|| not_sent(Error::new(ErrorClass::InvalidConfig)))?;
        let identifiers = if guard.ids.is_empty() {
            body.map_or(&[][..], SerializeBody::identifiers)
        } else {
            guard.ids
        };
        if path
            .iter()
            .chain(identifiers)
            .any(|segment| evidence.id_reflects(segment))
        {
            return Err(not_sent(input_error(
                "monitor request identifier withheld".into(),
            )));
        }
        let body = body
            .map(SerializeBody::encode)
            .transpose()
            .map_err(|_| {
                not_sent(input_error(
                    "monitor request body could not be encoded".into(),
                ))
            })?
            .map_or_else(Bytes::new, Bytes::from);
        let operation = Operation {
            id: mutation.id,
            template: mutation.route,
            method: (*mutation.method).clone(),
            class: Classification::Write,
            retry: mutation.retry,
            success: mutation.success,
            cap: mutation.limit,
            allow_recovery: !matches!(mutation.retry, Retry::Once),
            allow_empty: true,
            no_output: false,
            keep_status: mutation.keep_status,
            overflow_overrides_status: false,
            sensitive: mutation.private,
            reject_reflected_credentials: false,
            no_redirect: mutation.no_redirect,
            retry_config: retry::Config::default(),
        };
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| not_sent(*error))?;
        let bearer = self.session.bearer_source();
        let mut scoped =
            crate::quote::request::monitor::ScopedBearer::new(bearer, identifiers, mutation.id);
        if let Some(privacy) = guard.privacy {
            let mut values = evidence.credentials().clone();
            values.extend(privacy.credentials());
            if values.values().len() > 256 {
                return Err(not_sent(Error::new(ErrorClass::Api).with_outer_detail(
                    "monitor: credential safety limit exceeded".to_owned(),
                )));
            }
            scoped = scoped.with_privacy(values);
        }
        let bearer = if identifiers.is_empty() && guard.privacy.is_none() {
            bearer
        } else {
            &scoped
        };
        let response = execute_raw(
            runtime,
            base,
            &operation,
            &Options {
                dispatch_record: guard.dispatch,
                report_redirect: mutation.no_redirect || matches!(mutation.retry, Retry::Once),
                ..Options::DEFAULT
            },
            path,
            &[],
            RouteAuth::Bearer(bearer),
            body,
            evidence,
        )
        .await
        .map_err(|failure| {
            let (error, _, dispatch) = failure.into_parts();
            Failure::new(
                error.screen_api(&|text| evidence.text_reflects(text)),
                None,
                dispatch,
            )
        })?;
        let status = response.status;
        let body = Bytes::from(response.reply.body);
        let body = classify_full(
            mutation.id,
            status,
            body,
            mutation.success,
            (true, mutation.private, mutation.retry),
            evidence.credentials(),
        )
        .map_err(|error| maybe_sent(error.screen_api(&|text| evidence.text_reflects(text))))?;
        Ok(Reply {
            status: status.as_u16(),
            body: body.to_vec(),
        })
    }
}

pub(super) struct GuardedBody<'a, T> {
    pub(super) value: &'a T,
    pub(super) ids: &'a [&'a str],
}
impl<T: Serialize + Send + Sync> SerializeBody for GuardedBody<'_, T> {
    fn identifiers(&self) -> &[&str] {
        self.ids
    }
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self.value)
    }
}

pub(super) trait SerializeBody: Send + Sync {
    fn identifiers(&self) -> &[&str] {
        &[]
    }
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error>;
}
impl<T: Serialize + Send + Sync> SerializeBody for T {
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

pub(super) fn not_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::NotSent)
}
pub(super) fn maybe_sent<T>(error: Error) -> Failure<T> {
    Failure::new(error, None, WriteDispatch::MayHaveBeenSent)
}
pub(super) fn before_write<T>(failure: Failure<T>) -> Failure<T> {
    not_sent(failure.into_parts().0)
}

pub(super) fn screen_details(error: Error, evidence: &Evidence) -> Error {
    let mut error = error.screen_api(&|text| evidence.text_reflects(text));
    if error
        .detail::<String>()
        .is_some_and(|detail| evidence.text_reflects(detail))
    {
        error.remove_details::<String>(|detail| evidence.text_reflects(detail));
        let fixed = "monitor: request failed; details withheld";
        if !evidence.text_reflects(fixed) {
            error = error.with_outer_detail(fixed.to_owned());
        }
    }
    error
}
