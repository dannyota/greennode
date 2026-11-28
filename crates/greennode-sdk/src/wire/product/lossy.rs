use serde::de::DeserializeOwned;

use crate::model::wire::go_json;

pub(crate) fn with_normalized<T>(body: &[u8], decode: impl FnOnce(&str) -> T) -> T {
    decode(&go_json(body))
}

pub(crate) fn from_slice<T: DeserializeOwned>(body: &[u8]) -> serde_json::Result<T> {
    serde_json::from_str(&go_json(body))
}

pub(crate) fn validate(
    body: &[u8],
    limit: usize,
    validate: impl FnOnce(&[u8], usize) -> Result<(), crate::wire::strict_json::Error>,
) -> Result<(), crate::wire::strict_json::Error> {
    if body.len() > limit {
        return Err(crate::wire::strict_json::Error);
    }
    let text = go_json(body);
    validate(text.as_bytes(), text.len())
}

#[cfg(test)]
mod tests {
    use super::from_slice;
    use serde::Deserialize;
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    struct Raw {
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
    fn raw_values_use_the_normalized_input() {
        let input = b"{\"padding\":\"\xff\",\"raw\":{\"\\uD800\":\"\xe2\x82\\uDC00\"}}";
        let retained: Raw = from_slice(input).unwrap();
        assert_eq!(retained.raw.get(), r#"{"�":"���"}"#);
        let direct: Raw = serde_json::from_reader(br#"{"raw":"\uD800"}"#.as_slice()).unwrap();
        assert_eq!(direct.raw.get(), r#""\uD800""#);
    }
}
