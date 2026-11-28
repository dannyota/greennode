pub(crate) mod decode;
pub(crate) mod monitor;

use crate::config::session::Session;
use crate::wire::product::{
    DiscoveryDocument,
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::{Options, Pacing},
    redirect::{Classification, Retry},
    retry, screened_discovery_stream, sender,
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
struct Document {
    raw: Option<Box<RawValue>>,
    #[serde(skip)]
    raw_len: usize,
}

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

impl Output for Document {
    const SCHEMA: Shape<'static> = Shape::Any;
    const RESPONSE_GUARD: Option<sender::ResponseGuard> = Some(guard_discovery);

    fn decode_response(op: &Operation, response: &sender::Response) -> Result<Self, Box<Error>> {
        let status = response.status.as_u16();
        check_response(op, response)?;
        response.decode_projection(|body| {
            let text = crate::model::wire::go_json(body);
            let raw = serde_json::from_str::<Box<RawValue>>(&text).map_err(|_| {
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
            let peer_reflects = !response.reflection.is_clean();
            if body.len() > op.cap
                || strict_json::validate(text.as_bytes(), &[], text.len()).is_err()
                || peer_reflects
            {
                return Err(invalid(op.id, status));
            }
            Ok(Self {
                raw: Some(raw),
                raw_len: body.trim_ascii().len(),
            })
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(transparent)]
struct VnetworkDocument(Document);

impl Output for VnetworkDocument {
    const SCHEMA: Shape<'static> = Shape::Any;

    fn decode_response(op: &Operation, response: &sender::Response) -> Result<Self, Box<Error>> {
        if response.status == StatusCode::OK && response.reflection.clean().is_none() {
            return Err(invalid(op.id, response.status.as_u16()));
        }
        if response.status == StatusCode::OK {
            let receipt = response
                .clean_receipt()
                .ok_or_else(|| invalid(op.id, response.status.as_u16()))?;
            let normalized = crate::model::wire::go_json(receipt.body());
            if receipt.body().len() > op.cap {
                return Err(invalid(op.id, response.status.as_u16()));
            }
            strict_json::validate(normalized.as_bytes(), &[], normalized.len())
                .map_err(|_| invalid(op.id, response.status.as_u16()))?;
            let raw = serde_json::from_str::<Box<RawValue>>(&normalized)
                .map_err(|_| invalid(op.id, response.status.as_u16()))?;
            return Ok(Self(Document {
                raw: Some(raw),
                raw_len: receipt.body().trim_ascii().len(),
            }));
        }
        check_response(op, response)?;
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
    let mut evidence = Evidence::from_credentials(known);
    evidence.absorb(sent);
    (!evidence.go_peer_body_screen(body).is_clean()).then(|| invalid(op, status.as_u16()))
}

fn guard_catalogue(
    op: &'static str,
    status: StatusCode,
    body: &[u8],
    known: &sender::SentCredentials,
    sent: &sender::SentCredentials,
) -> Option<Box<Error>> {
    let normalized = screened_discovery_stream(body, known, sent, true);
    let unsafe_body = normalized.is_none_or(|body| {
        op == "pricing.GetQuote"
            && [known, sent].into_iter().any(|credentials| {
                crate::wire::product::response::reflects_credential_forms(&body, credentials)
            })
    });
    unsafe_body.then(|| invalid(op, status.as_u16()))
}

fn check_response(op: &Operation, response: &sender::Response) -> Result<(), Box<Error>> {
    if op.success.contains(&response.status) {
        return Ok(());
    }
    response
        .classify(op.id, op.success, (true, op.sensitive, op.retry), false)
        .map(drop)
}

pub(crate) async fn exchange(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
    req: Request<'_>,
    pacing: Option<&Pacing>,
) -> Result<Box<RawValue>, Box<Error>> {
    let operation = req.operation;
    exchange_validated(session, ctx, evidence, req, pacing, operation, |_| Ok(()))
        .await
        .map(|(raw, _)| raw)
}

pub(super) async fn exchange_validated(
    session: &Session,
    ctx: &RequestContext,
    evidence: &mut Evidence,
    req: Request<'_>,
    pacing: Option<&Pacing>,
    validation_op: &'static str,
    check_budget: impl FnOnce(usize) -> Result<(), Box<Error>>,
) -> Result<(Box<RawValue>, usize), Box<Error>> {
    let direct = pacing.is_none();
    let vnetwork = matches!(req.product, Product::Vnetwork(_));
    let pricing_quote =
        direct && req.operation == "pricing.GetQuote" && matches!(req.product, Product::Portal);
    let no_redirect = !direct
        || pricing_quote
        || vnetwork
        || matches!(
            req.product,
            Product::Monitor {
                no_redirect: true,
                ..
            }
        );
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
        response_guard: (!direct && !vnetwork).then_some(guard_catalogue),
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
        } else if direct {
            1 << 20
        } else {
            8 << 20
        },
        allow_recovery: true,
        allow_empty: false,
        no_output: false,
        keep_status: pricing_quote
            || (vnetwork
                && matches!(
                    req.operation,
                    "network.QuoteCreateEndpoint" | "network.CreateEndpoint"
                )),
        raw_body: false,
        overflow_overrides_status: false,
        sensitive: !direct,
        reflection: crate::wire::product::operation::Reflection::Redact,
        no_redirect,
        report_redirect: no_redirect,
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
        .await
        .and_then(|raw| {
            let raw_len = raw.get().len();
            check_budget(raw_len)?;
            Ok((raw, raw_len))
        });
    }
    let source = session.bearer_source();
    let (status, raw, raw_len) = if vnetwork {
        let (status, VnetworkDocument(document)) = operation::execute_with_evidence(
            runtime, base, &operation, &options, req.path, req.query, source, body, evidence,
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
        (status, document.raw, document.raw_len)
    } else if direct {
        let (status, document) = operation::execute_with_evidence::<Document>(
            runtime, base, &operation, &options, req.path, req.query, source, body, evidence,
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
        (status, document.raw, document.raw_len)
    } else {
        let (status, document) = operation::execute_with_evidence::<DiscoveryDocument>(
            runtime, base, &operation, &options, req.path, req.query, source, body, evidence,
        )
        .await
        .map_err(|failure| Box::new(failure.into_parts().0))?;
        (status, document.raw, document.raw_len)
    };
    check_budget(raw_len)?;
    let schema_error = || {
        Box::new(
            invalid(validation_op, status.as_u16())
                .screen_api(&|text| evidence.text_reflects(text)),
        )
    };
    let raw = raw.ok_or_else(schema_error)?;
    if !vnetwork && strict_json::validate(raw.get().as_bytes(), &[], raw.get().len()).is_err() {
        return Err(schema_error());
    }

    Ok((raw, raw_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Response, Server, with_product_loopback};

    #[tokio::test]
    async fn paced_catalog_reports_the_refused_redirect() {
        let server = Server::start(vec![Response::Raw(
            b"HTTP/1.1 302 Found\r\nLocation: /catalog\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
        )]);
        let options = crate::ConfigOptions::default()
            .with_region("hcm-3".into())
            .with_static_token(crate::Secret::new("toy-token"));
        let config = crate::Config::new(with_product_loopback(
            crate::testing::Product::Vserver,
            options,
            &server,
        ))
        .unwrap();
        let (context, _) = RequestContext::new();
        let error = crate::quote::catalog::Catalog::new(config.session(), &context, 4)
            .regional(
                "portal.ListZones",
                "portal.ListZones",
                "v1/{}/zones",
                &["project-1"],
                &[],
            )
            .await
            .unwrap_err();
        let api = error.api_error().unwrap();
        assert_eq!(
            (api.status(), api.code(), api.message()),
            (Some(302), "", "Found")
        );
        assert_eq!(server.requests(), 1);
    }
}
