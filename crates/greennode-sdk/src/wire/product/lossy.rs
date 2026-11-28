use std::{cell::RefCell, collections::BTreeMap};

use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::value::RawValue;
use zeroize::Zeroizing;

use crate::model::wire::{go_utf8, replace_surrogates};

struct Source {
    original: Zeroizing<String>,
    normalized_start: usize,
}

thread_local! {
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
}

struct Restore(Option<Source>);

impl Drop for Restore {
    fn drop(&mut self) {
        SOURCE.set(self.0.take());
    }
}

pub(crate) fn with_normalized<T>(body: &[u8], decode: impl FnOnce(&str) -> T) -> T {
    let original = go_utf8(body);
    let Some(normalized) = replace_surrogates(&original, "\\ufffd") else {
        return decode(&original);
    };
    let normalized = Zeroizing::new(normalized);
    let source = Source {
        original: Zeroizing::new(original.into_owned()),
        normalized_start: normalized.as_ptr().addr(),
    };
    let _restore = Restore(SOURCE.replace(Some(source)));
    decode(&normalized)
}

pub(crate) fn from_slice<T: DeserializeOwned>(body: &[u8]) -> serde_json::Result<T> {
    with_normalized(body, |text| serde_json::from_str(text))
}

pub(crate) fn validate(
    body: &[u8],
    limit: usize,
    validate: impl FnOnce(&[u8], usize) -> Result<(), crate::wire::strict_json::Error>,
) -> Result<(), crate::wire::strict_json::Error> {
    if body.len() > limit {
        return Err(crate::wire::strict_json::Error);
    }
    let text = crate::model::wire::go_json(body);
    validate(text.as_bytes(), text.len())
}

pub(crate) fn boxed<'de, D: Deserializer<'de>>(decoder: D) -> Result<Box<RawValue>, D::Error> {
    if SOURCE.with_borrow(Option::is_some) {
        to_owned(<&RawValue>::deserialize(decoder)?).map_err(serde::de::Error::custom)
    } else {
        Box::<RawValue>::deserialize(decoder)
    }
}

pub(crate) fn optional_boxed<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Present(#[serde(deserialize_with = "boxed")] Box<RawValue>);

    Option::<Present>::deserialize(decoder).map(|value| value.map(|value| value.0))
}

pub(crate) fn present_boxed<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    boxed(decoder).map(Some)
}

pub(crate) fn borrowed<'de, D: Deserializer<'de>>(decoder: D) -> Result<&'de RawValue, D::Error> {
    <&RawValue>::deserialize(decoder)
}

pub(crate) fn optional_borrowed<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<&'de RawValue>, D::Error> {
    Option::<&RawValue>::deserialize(decoder)
}

pub(crate) fn borrowed_map<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<BTreeMap<String, &'de RawValue>, D::Error> {
    BTreeMap::deserialize(decoder)
}

pub(crate) fn to_owned(raw: &RawValue) -> serde_json::Result<Box<RawValue>> {
    SOURCE.with_borrow(|source| {
        let normalized = raw.get();
        let original = source.as_ref().and_then(|source| {
            let start = normalized
                .as_ptr()
                .addr()
                .checked_sub(source.normalized_start)?;
            let end = start.checked_add(normalized.len())?;
            source.original.get(start..end)
        });
        match original {
            Some(original) if original != normalized => RawValue::from_string(original.to_owned()),
            _ => Ok(raw.to_owned()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Raw {
        #[serde(deserialize_with = "boxed")]
        raw: Box<RawValue>,
    }

    #[test]
    fn lossy_strings_reach_maps_enums_untagged_and_flattened_values() {
        #[derive(Debug, Deserialize, PartialEq)]
        enum Choice {
            #[serde(rename = "�")]
            Replacement,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Untagged {
            Text(String),
            Object(serde_json::Value),
        }

        #[derive(Deserialize)]
        struct Probe {
            choice: Choice,
            rows: BTreeMap<String, Untagged>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }

        let input = b"{\"choice\":\"\xff\",\"rows\":{\"\xff\":\"\\ud800\",\"obj\":{\"\\udc00\":\"\\ud800\\udfff\"}},\"\\ud801\":\"\xe2\x82\"}";
        let probe: Probe = from_slice(input).unwrap();
        assert_eq!(probe.choice, Choice::Replacement);
        assert!(matches!(&probe.rows["�"], Untagged::Text(value) if value == "�"));
        assert!(matches!(&probe.rows["obj"], Untagged::Object(value) if value["�"] == "\u{103ff}"));
        assert_eq!(probe.extra["�"], "��");
        assert!(from_slice::<serde_json::Value>(b"[\xff]").is_err());
        assert!(from_slice::<serde_json::Value>(b"\"\\\xff\"").is_err());
    }

    #[test]
    fn raw_values_preserve_escape_spelling_and_replace_invalid_bytes() {
        let input = b"{\"padding\":\"\xff\",\"raw\":{\"\\uD800\":\"\xe2\x82\\uDC00\"}}";
        let retained: Raw = from_slice(input).unwrap();
        assert_eq!(retained.raw.get(), r#"{"\uD800":"��\uDC00"}"#);
        let direct: Raw = serde_json::from_reader(br#"{"raw":"\uD800"}"#.as_slice()).unwrap();
        assert_eq!(direct.raw.get(), r#""\uD800""#);
    }

    #[test]
    fn nested_decodes_and_unwinding_restore_the_previous_raw_source() {
        let original = br#"{"raw":"\uD800"}"#;
        with_normalized(original, |text| {
            let nested: Raw = from_slice(br#"{"raw":"\udc00"}"#).unwrap();
            assert_eq!(nested.raw.get(), r#""\udc00""#);
            let panic = std::panic::catch_unwind(|| {
                with_normalized(br#""\ud800""#, |_| panic!("probe"));
            });
            assert!(panic.is_err());
            let outer: Raw = serde_json::from_str(text).unwrap();
            assert_eq!(outer.raw.get(), r#""\uD800""#);
        });
        assert!(SOURCE.with_borrow(Option::is_none));
    }
}
