use super::{Evidence, identifier_key};
use serde::Deserializer;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;
use zeroize::Zeroizing;

pub(super) fn decoded_reflects(evidence: &Evidence, body: &[u8]) -> Option<bool> {
    let raw: &RawValue = serde_json::from_slice(body).ok()?;
    Walker {
        evidence,
        identifier: false,
        depth: 0,
    }
    .check(raw)
    .ok()
}

struct Walker<'a> {
    evidence: &'a Evidence,
    identifier: bool,
    depth: usize,
}

impl Walker<'_> {
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
                Ok(self.evidence.string_reflects(&decoded, self.identifier))
            }
            Some(b'-' | b'0'..=b'9') => Ok(self.evidence.string_reflects(raw, self.identifier)),
            _ => Ok(self.evidence.text_reflects(raw)),
        }
    }

    fn child(&self, identifier: bool) -> Self {
        Self {
            evidence: self.evidence,
            identifier,
            depth: self.depth + 1,
        }
    }
}

impl<'de> Visitor<'de> for Walker<'_> {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object or array")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<bool, M::Error> {
        let mut reflects = false;
        while let Some((key, raw)) = map.next_entry::<String, &RawValue>()? {
            let key = Zeroizing::new(key);
            if !reflects {
                reflects = self.evidence.string_reflects(&key, self.identifier)
                    || self
                        .child(self.identifier || identifier_key(&key))
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
