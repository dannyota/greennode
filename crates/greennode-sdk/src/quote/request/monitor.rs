use std::{future::Future, pin::Pin};

use serde_json::value::RawValue;

use super::{Request, check_response};
use crate::endpoint::Base;
use crate::model::wire::go_json;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::Options,
    sender::{self, BearerSource, BearerToken},
};
use crate::wire::strict_json::{Shape, ShapeField, validate_canonical_shape};
use crate::{ApiError, Error, ErrorClass, RequestContext, Secret, error::SafeText};

struct ScopedBearer<'a> {
    source: &'a dyn BearerSource,
    identifiers: &'a [&'a str],
    operation: &'static str,
}

impl BearerSource for ScopedBearer<'_> {
    fn refreshable(&self) -> bool {
        self.source.refreshable()
    }

    fn token<'a>(
        &'a self,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<BearerToken, Error>> + Send + 'a>> {
        Box::pin(async move {
            let token = self.source.token(ctx).await?;
            let mut evidence = Evidence::new();
            if let Some(credentials) = token.signin_credentials.as_deref() {
                evidence.absorb(credentials);
            }
            evidence.add_secret(&token.token.access_token);
            if self.identifiers.iter().any(|id| evidence.id_reflects(id)) {
                return Err(evidence.screen_error(credential_conflict(self.operation)));
            }
            Ok(token)
        })
    }

    fn rejected<'a>(
        &'a self,
        ctx: &'a RequestContext,
        sent: &'a Secret,
        evict: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.source.rejected(ctx, sent, evict)
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(transparent)]
struct MonitorDocument(Option<Box<RawValue>>);

impl Output for MonitorDocument {
    const SCHEMA: Shape<'static> = Shape::Object(&[ShapeField {
        name: "optimumPrice",
        shape: Shape::Any,
    }]);

    fn decode_response(op: &Operation, response: &sender::Response) -> Result<Self, Box<Error>> {
        check_response(op, response)?;
        response.decode_projection(|body| {
            if body.is_empty() {
                return Err(Box::new(invalid(op.id)));
            }
            let text = go_json(body);
            let raw: Box<RawValue> = serde_json::from_str(&text).map_err(|_| {
                Box::new(error(
                    op.id,
                    Some(200),
                    "InvalidResponse",
                    "quota response withheld",
                    false,
                ))
            })?;
            validate_canonical_shape(raw.get().as_bytes(), &Self::SCHEMA, raw.get().len())
                .map_err(|_| Box::new(invalid(op.id)))?;
            price(&raw).ok_or_else(|| Box::new(invalid(op.id)))?;
            Ok(Self(Some(raw)))
        })
    }
}

pub(crate) fn price(raw: &RawValue) -> Option<f64> {
    #[derive(serde::Deserialize)]
    struct Price<'a> {
        #[serde(rename = "optimumPrice", borrow)]
        value: &'a RawValue,
    }
    let wire: Price<'_> = serde_json::from_str(raw.get()).ok()?;
    let text = wire.value.get();
    if !matches!(text.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
        return None;
    }
    let price = text.parse::<f64>().ok()?;
    let mantissa = text.split(['e', 'E']).next()?;
    (price.is_finite()
        && price >= 0.0
        && (price != 0.0 || mantissa.trim_matches(['-', '.', '0']).is_empty()))
    .then_some(price)
}

pub(super) async fn exchange(
    runtime: sender::Runtime<'_>,
    base: &Base,
    operation: &Operation,
    req: Request<'_>,
    body: bytes::Bytes,
    evidence: &mut Evidence,
    auth: (&dyn BearerSource, &[&str]),
) -> Result<Box<RawValue>, Box<Error>> {
    let (source, identifiers) = auth;
    if let Some(configured) = runtime.configured_credentials {
        evidence.absorb(configured);
    }
    if identifiers.iter().any(|id| evidence.id_reflects(id)) {
        return Err(Box::new(
            evidence.screen_error(credential_conflict(operation.id)),
        ));
    }
    let source = ScopedBearer {
        source,
        identifiers,
        operation: operation.id,
    };
    let result = operation::execute_with_evidence::<MonitorDocument>(
        runtime,
        base,
        operation,
        &Options::DEFAULT,
        req.path,
        req.query,
        &source,
        body,
        evidence,
    )
    .await;
    match result {
        Ok((_, output)) => output.0.ok_or_else(|| Box::new(invalid(operation.id))),
        Err(failure) => Err(Box::new(failure_error(
            operation.id,
            failure.into_parts().0,
            evidence,
        ))),
    }
}

fn failure_error(op: &'static str, failure: Error, evidence: &Evidence) -> Error {
    let api = failure.api_error();
    let status = api.and_then(ApiError::observed_status);
    let result = if failure.is_body_too_large() {
        error(
            op,
            status,
            "ResponseTooLarge",
            "quota response exceeded size limit",
            false,
        )
        .with_body_too_large()
    } else if operation::is_active_transport_failure(&failure) {
        crate::monitor::response_read_failure(
            failure,
            error(
                op,
                status,
                "InvalidResponse",
                "quota response withheld",
                false,
            ),
        )
    } else if matches!(
        failure.class(),
        ErrorClass::Cancelled | ErrorClass::DeadlineExceeded
    ) {
        failure
    } else if let Some(api) =
        api.filter(|api| api.is_for_operation(op) && api.code() != "InvalidResponse")
    {
        let retryable =
            api.retryable() && status.is_none_or(|status| matches!(status, 429 | 502 | 503 | 504));
        let code = if status.is_some_and(|status| status >= 400) {
            ""
        } else {
            "InvalidResponse"
        };
        error(op, status, code, "quota response withheld", retryable)
    } else {
        failure
    };
    evidence.screen_error(result)
}

fn invalid(op: &'static str) -> Error {
    error(
        op,
        Some(200),
        "InvalidResponse",
        "quota response had invalid structure",
        false,
    )
}

fn error(
    op: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::new(
        op,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

fn credential_conflict(op: &'static str) -> Error {
    error(
        op,
        None,
        "InvalidResponse",
        "quota response had invalid structure",
        false,
    )
}
