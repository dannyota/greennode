use super::{identifier_key, response::BodyMatcher};
use serde::Deserializer;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;
use zeroize::Zeroizing;

pub(super) fn decoded_reflects(matcher: &BodyMatcher<'_>, body: &[u8]) -> Option<bool> {
    check_decoded(body, false, identifier_key, &|text, identifier, _| {
        matcher.reflects(text.as_bytes(), identifier)
    })
}

pub(super) fn request_identifier_key(key: &str) -> bool {
    identifier_key(key) || matches!(key, "policyIdList" | "serverIdList")
}

pub(super) fn request_reflects(matcher: &BodyMatcher<'_>, body: &[u8], identifier: bool) -> bool {
    check_decoded(
        body,
        identifier,
        request_identifier_key,
        &|text, identifier, _| matcher.reflects(text.as_bytes(), identifier),
    ) == Some(true)
}

pub(super) fn output_reflects(evidence: &super::Evidence, body: &[u8]) -> Option<bool> {
    let matcher = BodyMatcher::for_output(evidence.credentials(), body.len());
    output_with_matcher(&matcher, body)
}

pub(super) fn output_with_matcher(matcher: &BodyMatcher<'_>, body: &[u8]) -> Option<bool> {
    if matcher.reflects(body, false) {
        return Some(true);
    }
    check_decoded(body, false, identifier_key, &|text, identifier, number| {
        matcher.reflects(text.as_bytes(), identifier)
            || (number && output_number_reflects(matcher, text, identifier))
    })
}

pub(super) fn output_identifier_reflects(matcher: &BodyMatcher<'_>, body: &[u8]) -> Option<bool> {
    check_decoded(body, true, identifier_key, &|text, identifier, number| {
        matcher.reflects(text.as_bytes(), identifier)
            || (number && output_number_reflects(matcher, text, identifier))
    })
}

fn output_number_reflects(matcher: &BodyMatcher<'_>, text: &str, identifier: bool) -> bool {
    let Ok(number) = text.parse::<f64>() else {
        return false;
    };
    if !identifier
        && text.parse::<i64>().is_err()
        && text.parse::<u64>().is_err()
        && serde_json::to_string(&number).is_ok_and(|canonical| canonical != text)
    {
        return false;
    }
    crate::output_number::any_spelling(number, false, |spelling| {
        matcher.reflects(spelling.as_bytes(), identifier)
    })
}

fn check_decoded<F: Fn(&str, bool, bool) -> bool>(
    body: &[u8],
    root_identifier: bool,
    identifier_key: fn(&str) -> bool,
    matches: &F,
) -> Option<bool> {
    let raw: &RawValue = serde_json::from_slice(body).ok()?;
    Walker {
        matches,
        identifier_key,
        identifier: root_identifier,
        depth: 0,
    }
    .check(raw)
    .ok()
}

struct Walker<'a, F> {
    matches: &'a F,
    identifier_key: fn(&str) -> bool,
    identifier: bool,
    depth: usize,
}

impl<F: Fn(&str, bool, bool) -> bool> Walker<'_, F> {
    fn check(self, value: &RawValue) -> Result<bool, serde_json::Error> {
        let raw = value.get();
        let mut decoder = serde_json::Deserializer::from_str(raw);
        match raw.as_bytes().first() {
            Some(b'{' | b'[') if self.depth >= 128 => {
                Err(serde::de::Error::custom("JSON depth limit"))
            }
            Some(b'{') => decoder.deserialize_map(self),
            Some(b'[') => decoder.deserialize_seq(self),
            Some(b'"') => {
                let decoded = Zeroizing::new(serde_json::from_str::<String>(raw)?);
                Ok((self.matches)(&decoded, self.identifier, false))
            }
            Some(b'-' | b'0'..=b'9') => Ok((self.matches)(raw, self.identifier, true)),
            _ => Ok((self.matches)(raw, false, false)),
        }
    }

    fn child(&self, identifier: bool) -> Self {
        Self {
            matches: self.matches,
            identifier_key: self.identifier_key,
            identifier,
            depth: self.depth + 1,
        }
    }
}

impl<'de, F: Fn(&str, bool, bool) -> bool> Visitor<'de> for Walker<'_, F> {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object or array")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<bool, M::Error> {
        let mut reflects = false;
        while let Some((key, raw)) = map.next_entry::<String, &RawValue>()? {
            let key = Zeroizing::new(key);
            if !reflects {
                reflects = (self.matches)(&key, self.identifier, false)
                    || self
                        .child(self.identifier || (self.identifier_key)(&key))
                        .check(raw)
                        .map_err(serde::de::Error::custom)?;
            }
        }
        Ok(reflects)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
        let mut reflects = false;
        while let Some(raw) = seq.next_element::<&RawValue>()? {
            if !reflects {
                reflects = self
                    .child(self.identifier)
                    .check(raw)
                    .map_err(serde::de::Error::custom)?;
            }
        }
        Ok(reflects)
    }
}
