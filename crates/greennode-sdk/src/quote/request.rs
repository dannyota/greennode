pub(crate) mod decode;
pub(crate) mod monitor;

use crate::config::session::Session;
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::{Options, Pacing},
    redirect::{Classification, Retry},
    retry, sender,
};
use crate::wire::strict_json::{self, Shape};
use crate::{ApiError, Error, ErrorClass, RequestContext, error::SafeText};
use bytes::Bytes;
use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

#[derive(Clone, Copy)]
pub(crate) enum Product<'a> {
    Monitor {
        identifiers: &'a [&'a str],
        no_redirect: bool,
    },
    Portal,
    Regional,
    Vnetwork(&'a crate::endpoint::Base),
    Storage(Option<&'a str>),
}

impl<'a> Product<'a> {
    pub(crate) fn endpoint<'s>(self, session: &'s Session) -> Option<&'s crate::endpoint::Base>
    where
        'a: 's,
    {
        let endpoints = session.endpoints();
        match self {
            Self::Monitor { .. } => endpoints.monitor.as_ref(),
            Self::Portal => endpoints.portal.as_ref(),
            Self::Regional => endpoints.vserver.as_ref(),
            Self::Vnetwork(base) => Some(base),
            Self::Storage(_) => endpoints.storage.as_ref(),
        }
    }
}

pub(crate) struct Request<'a> {
    pub(crate) operation: &'static str,
    pub(crate) template: &'static str,
    pub(crate) product: Product<'a>,
    pub(crate) path: &'a [&'a str],
    pub(crate) query: &'a [(&'a str, &'a str)],
    pub(crate) body: Option<&'a Value>,
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub(crate) struct Document(pub(crate) Option<Box<RawValue>>);

pub(crate) fn api(
    operation: &'static str,
    status: Option<u16>,
    code: &str,
    message: &str,
    retryable: bool,
) -> Error {
    Error::from_api(ApiError::without_fallback(
        operation,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        retryable,
    ))
}

pub(crate) fn invalid(operation: &'static str, status: u16) -> Box<Error> {
    Box::new(api(
        operation,
        Some(status),
        "InvalidResponse",
        "invalid discovery response; body withheld",
        false,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteProperties<'a> {
    #[serde(borrow)]
    properties_price: Option<Vec<Option<QuoteProperty<'a>>>>,
}

#[derive(Deserialize)]
struct QuoteProperty<'a> {
    #[serde(borrow)]
    name: Option<&'a RawValue>,
    #[serde(borrow)]
    description: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct StorageQuote<'a> {
    #[serde(borrow)]
    data: Option<&'a RawValue>,
}

fn screen_properties(raw: &RawValue, evidence: &Evidence, storage: bool) -> bool {
    let raw = if storage {
        let Ok(StorageQuote { data: Some(data) }) = serde_json::from_str(raw.get()) else {
            return false;
        };
        data
    } else {
        raw
    };
    serde_json::from_str::<QuoteProperties<'_>>(raw.get()).is_ok_and(|quote| {
        quote
            .properties_price
            .into_iter()
            .flatten()
            .flatten()
            .any(|row| {
                [row.name, row.description]
                    .into_iter()
                    .flatten()
                    .any(|text| {
                        serde_json::from_str::<String>(text.get())
                            .is_ok_and(|text| evidence.text_reflects(&text))
                    })
            })
    })
}

impl Output for Document {
    const SCHEMA: Shape<'static> = Shape::Any;
    const RESPONSE_GUARD: Option<sender::ResponseGuard> = Some(guard_discovery);

    fn decode_response(op: &Operation, response: &sender::Response) -> Result<Self, Box<Error>> {
        let status = response.status.as_u16();
        let body = response_body(op, response)?;
        let raw = serde_json::from_slice::<Box<RawValue>>(&body).map_err(|_| {
            if op.id == "pricing.GetQuote" {
                return invalid(op.id, status);
            }
            Box::new(api(
                op.id,
                None,
                "",
                "response failed to decode; body withheld",
                false,
            ))
        })?;
        if strict_json::validate(&body, &[], op.cap).is_err()
            || operation::reflects_json(&body, &response.credentials, true).unwrap_or(true)
        {
            return Err(invalid(op.id, status));
        }
        Ok(Self(Some(raw)))
    }
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
struct VnetworkDocument(Document);

impl Output for VnetworkDocument {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(op: &Operation, response: &sender::Response) -> Result<Self, Box<Error>> {
        let normalized = crate::model::wire::go_json(&response.body);
        if response.status == StatusCode::OK {
            strict_json::validate(normalized.as_bytes(), &[], op.cap)
                .map_err(|_| invalid(op.id, response.status.as_u16()))?;
            let raw = serde_json::from_str::<Box<RawValue>>(&normalized)
                .map_err(|_| invalid(op.id, response.status.as_u16()))?;
            return Ok(Self(Document(Some(raw))));
        }
        response_body(op, response)?;
        Err(invalid(op.id, response.status.as_u16()))
    }
}

fn guard_discovery(
    op: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &sender::SentCredentials,
    sent: &sender::SentCredentials,
) -> Option<Box<Error>> {
    if op == "pricing.GetQuote" {
        return None;
    }
    guard_exchange(op, status, body, known, sent)
}

fn guard_exchange(
    op: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &sender::SentCredentials,
    sent: &sender::SentCredentials,
) -> Option<Box<Error>> {
    let unsafe_body = operation::unsafe_json_response(body, known, sent, true)
        || (op == "pricing.GetQuote"
            && std::str::from_utf8(body).is_ok_and(|body| {
                [known, sent].into_iter().any(|credentials| {
                    crate::wire::product::response::reflects_credential_forms(body, credentials)
                })
            }));
    unsafe_body.then(|| invalid(op, status.as_u16()))
}

fn response_body(op: &Operation, response: &sender::Response) -> Result<Bytes, Box<Error>> {
    crate::wire::product::response::classify(
        op.id,
        response.status,
        Bytes::copy_from_slice(&response.body),
        op.success,
        (true, op.sensitive, op.retry),
        &response.credentials,
    )
}

pub(crate) async fn exchange(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
    req: Request<'_>,
    pacing: Option<&Pacing>,
) -> Result<Box<RawValue>, Box<Error>> {
    let direct = pacing.is_none();
    let monitor_log =
        matches!(req.product, Product::Monitor { .. }) && req.operation == monitor::LOG_QUOTE;
    let vnetwork = matches!(req.product, Product::Vnetwork(_));
    let base = req
        .product
        .endpoint(session)
        .ok_or_else(|| Box::new(Error::new(ErrorClass::InvalidConfig)))?;
    let region = match req.product {
        Product::Storage(region) => region,
        _ => None,
    };
    let headers = region.map(|id| [("region", id), ("region_id", id)]);
    let options = Options {
        pacing,
        response_guard: (!direct && !vnetwork).then_some(guard_exchange),
        headers: headers.as_ref().map_or(&[], |headers| headers.as_slice()),
        ..Options::DEFAULT
    };
    let operation = Operation {
        id: req.operation,
        template: req.template,
        method: if req.body.is_some() {
            Method::POST
        } else {
            Method::GET
        },
        class: Classification::Read,
        retry: Retry::Repeatable,
        success: &[StatusCode::OK],
        cap: if vnetwork {
            4 << 20
        } else if direct && !monitor_log {
            1 << 20
        } else {
            8 << 20
        },
        allow_recovery: true,
        allow_empty: false,
        no_output: false,
        keep_status: !monitor_log,
        overflow_overrides_status: false,
        sensitive: !direct,
        reject_reflected_credentials: false,
        no_redirect: match req.product {
            Product::Monitor { no_redirect, .. } => no_redirect,
            _ => true,
        },
        retry_config: retry::Config::default(),
    };
    let body = req
        .body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| Box::new(Error::new(ErrorClass::InvalidInput)))?
        .map_or_else(Bytes::new, Bytes::from);
    let mut jitter = || session.retry_jitter();
    let runtime = session.runtime(ctx, &mut jitter)?;
    if let Product::Monitor { identifiers, .. } = req.product {
        return monitor::exchange(
            runtime,
            base,
            &operation,
            req,
            body,
            evidence,
            (session.bearer_source(), identifiers),
        )
        .await;
    }
    let source = session.bearer_source();
    let (status, document) = if vnetwork {
        let (status, VnetworkDocument(document)) = operation::execute_with_evidence(
            runtime, base, &operation, &options, req.path, req.query, source, body, evidence,
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
        (status, document)
    } else {
        operation::execute_with_evidence::<Document>(
            runtime, base, &operation, &options, req.path, req.query, source, body, evidence,
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?
    };
    let invalid = || invalid(req.operation, status.as_u16());
    let raw = document.0.ok_or_else(invalid)?;
    if !vnetwork
        && req.body.is_some()
        && screen_properties(&raw, evidence, matches!(req.product, Product::Storage(_)))
    {
        return Err(invalid());
    }
    Ok(raw)
}
