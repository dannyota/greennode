pub(crate) mod absence;
pub(crate) mod evidence;
pub(crate) mod exit;
pub(crate) mod guard;
pub(crate) mod identifier;
pub(crate) mod lossy;
pub(crate) mod operation;
pub(crate) mod options;
pub(crate) mod output;
pub(crate) mod redirect;
pub(crate) mod response;
pub(crate) mod retry;
pub(crate) mod sender;
pub(crate) mod unauthorized;

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::{ApiError, Error, error::SafeText};
use operation::{Operation, Output};
use sender::{Response, SentCredentials};

pub(crate) fn screened_discovery_stream<'a>(
    body: &'a [u8],
    known: &SentCredentials,
    sent: &SentCredentials,
    check_syntax: bool,
) -> Option<Cow<'a, str>> {
    if known.reflects(body) || sent.reflects(body) {
        return None;
    }
    let normalized = crate::model::wire::go_json(body);
    if check_syntax && matches!(normalized, Cow::Owned(_)) && !crate::go_json::valid_stream(body) {
        return None;
    }
    let mut evidence = evidence::Evidence::from_credentials(known);
    evidence.absorb(sent);
    evidence
        .normalized_discovery_body_screen(body, &normalized, check_syntax)
        .is_clean()
        .then_some(normalized)
}

#[derive(Default, Deserialize)]
pub(crate) struct DiscoveryDocument {
    #[serde(
        default,
        deserialize_with = "crate::wire::product::lossy::optional_boxed"
    )]
    pub(crate) raw: Option<Box<RawValue>>,
    #[serde(skip)]
    pub(crate) raw_len: usize,
    #[serde(skip)]
    pub(crate) credentials: SentCredentials,
}

impl Output for DiscoveryDocument {
    const SCHEMA: crate::wire::strict_json::Shape<'static> = crate::wire::strict_json::Shape::Any;

    fn decode_response(op: &Operation, reply: &Response) -> Result<Self, Box<Error>> {
        reply.classify(op.id, op.success, (true, op.sensitive, op.retry), false)?;
        reply.decode_projection(|body| {
            if body.is_empty() {
                return Ok(Self::default());
            }
            let invalid = || {
                Box::new(Error::from_api(ApiError::without_fallback(
                    op.id,
                    Some(reply.status.as_u16()),
                    SafeText::new("InvalidResponse", &[]),
                    SafeText::new("response failed to decode; body withheld", &[]),
                    false,
                )))
            };
            let raw = crate::go_json::raw_message(body)
                .map_err(|()| invalid())?
                .ok_or_else(invalid)?;
            let normalized = crate::model::wire::go_json(raw);
            let document = serde_json::from_str(&normalized).map_err(|_| invalid())?;
            Ok(Self {
                raw: Some(document),
                raw_len: raw.len(),
                credentials: reply.credentials.clone(),
            })
        })
    }
}
