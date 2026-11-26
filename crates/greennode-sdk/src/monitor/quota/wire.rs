use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::super::{
    Client,
    wire::{Family, Read},
};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{field_matches, validate};
use crate::{Error, RequestContext};

const COMPOSITE_LIMIT: usize = 8 << 20;
const CREDENTIAL_LIMIT: usize = 256;

pub(super) struct Reader<'a> {
    client: &'a Client,
    ctx: &'a RequestContext,
    retained: usize,
    exhausted: bool,
    pub(super) decision_ids: Vec<String>,
    pub(super) read: Read,
    pub(super) evidence: Evidence,
}

impl<'a> Reader<'a> {
    pub(super) fn new(client: &'a Client, ctx: &'a RequestContext, id: &'static str) -> Self {
        Self {
            client,
            ctx,
            retained: 0,
            exhausted: false,
            decision_ids: Vec::new(),
            read: Read {
                id,
                route: "",
                limit: 1 << 20,
                family: Family::Quota,
            },
            evidence: Evidence::new(),
        }
    }

    pub(super) async fn get(
        &mut self,
        route: &'static str,
        segments: &[&str],
    ) -> Result<Box<RawValue>, Error> {
        if self.exhausted {
            return Err(self
                .read
                .error(
                    None,
                    "RequestFailed",
                    "quota retained body budget exhausted",
                    false,
                    &self.evidence,
                )
                .with_body_too_large());
        }
        let read = Read { route, ..self.read };
        let response = self
            .client
            .read(self.ctx, &read, segments, &[], &mut self.evidence)
            .await;
        if self.evidence.credentials().values().len() > CREDENTIAL_LIMIT {
            self.exhausted = true;
            return Err(read.error(
                response.as_ref().map_or_else(
                    |error| error.api_error().and_then(crate::ApiError::status),
                    |_| Some(200),
                ),
                "InvalidResponse",
                "quota credential safety limit exceeded",
                false,
                &self.evidence,
            ));
        }
        let raw = response?;
        validate(raw.get().as_bytes(), &[], raw.get().len()).map_err(|_| self.invalid())?;
        let length = sanitized_len(raw.get());
        if length > COMPOSITE_LIMIT - self.retained {
            self.exhausted = true;
            return Err(read
                .error(
                    Some(200),
                    "ResponseTooLarge",
                    "quota retained body budget exceeded",
                    false,
                    &self.evidence,
                )
                .with_body_too_large());
        }
        self.retained += length;
        Ok(raw)
    }

    pub(super) fn invalid(&self) -> Error {
        self.read.invalid(&self.evidence)
    }

    pub(super) fn decode<T>(&self, decoded: Option<T>) -> Result<T, Error> {
        decoded.ok_or_else(|| self.invalid())
    }
}

fn sanitized_len(raw: &str) -> usize {
    let mut in_string = false;
    let mut escaped = false;
    let mut length = 0;
    for ch in raw.chars() {
        if in_string {
            length += match ch {
                '<' | '>' | '&' if !escaped => 6,
                '\u{2028}' | '\u{2029}' if !escaped => 6,
                _ => ch.len_utf8(),
            };
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if !ch.is_ascii_whitespace() {
            length += ch.len_utf8();
            in_string = ch == '"';
        }
    }
    length
}

pub(super) struct Object<'a>(BTreeMap<String, &'a RawValue>);

impl<'a> Object<'a> {
    pub(super) fn of(raw: &'a RawValue) -> Option<Self> {
        serde_json::from_str(raw.get()).ok().map(Self)
    }

    pub(super) fn field(&self, key: &str) -> Option<Option<&'a RawValue>> {
        if self
            .0
            .keys()
            .any(|name| name != key && field_matches(name, key))
        {
            return None;
        }
        Some(self.0.get(key).copied())
    }

    pub(super) fn required(&self, key: &str) -> Option<&'a RawValue> {
        self.field(key)?
    }

    pub(super) fn text(&self, key: &str) -> Option<String> {
        token(self.required(key)?)
    }

    pub(super) fn integer(&self, key: &str) -> Option<i64> {
        integer(self.required(key)?)
    }

    pub(super) fn price(&self) -> Option<Option<f64>> {
        match self.field("price")?.map(RawValue::get) {
            None | Some("null") => Some(None),
            Some(raw) if number_token(raw) => {
                let value = raw.parse::<f64>().ok()?;
                let mantissa = raw.split(['e', 'E']).next()?;
                (value.is_finite()
                    && value >= 0.0
                    && (value != 0.0 || mantissa.trim_matches(['-', '0', '.']).is_empty()))
                .then_some(Some(value))
            }
            Some(_) => None,
        }
    }
}

pub(super) fn token<T: DeserializeOwned>(raw: &RawValue) -> Option<T> {
    serde_json::from_str(raw.get()).ok()
}

pub(super) fn array(raw: &RawValue) -> Option<Vec<&RawValue>> {
    serde_json::from_str(raw.get()).ok()
}

fn number_token(raw: &str) -> bool {
    matches!(raw.as_bytes().first(), Some(b'-' | b'0'..=b'9'))
}

pub(super) fn integer(value: &RawValue) -> Option<i64> {
    let raw = value.get();
    if raw.len() > 128 || !number_token(raw) {
        return None;
    }
    let (mantissa, exponent) = match raw.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (raw, 0),
    };
    if !(-1024..=1024).contains(&exponent) {
        return None;
    }
    let (negative, mantissa) = mantissa
        .strip_prefix('-')
        .map_or((false, mantissa), |value| (true, value));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    let scale = exponent - i32::try_from(fraction.len()).ok()?;
    let significant = if scale < 0 {
        let length = digits.len().checked_sub(scale.unsigned_abs() as usize)?;
        let (number, rest) = digits.split_at(length);
        if rest.bytes().any(|digit| digit != b'0') {
            return None;
        }
        number
    } else {
        digits
    };
    let magnitude = significant
        .parse::<u64>()
        .ok()?
        .checked_mul(10_u64.checked_pow(scale.max(0) as u32)?)?;
    let signed = if negative {
        -i128::from(magnitude)
    } else {
        i128::from(magnitude)
    };
    i64::try_from(signed).ok()
}
