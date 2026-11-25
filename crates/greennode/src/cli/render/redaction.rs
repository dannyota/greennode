use serde::Serialize;

pub(super) const MARKER: &str = "$greennode::dynamic-map";

pub struct DynamicMap<T>(pub T);

impl<T: Serialize> Serialize for DynamicMap<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(MARKER, &self.0)
    }
}

pub(super) fn sensitive(key: &str) -> bool {
    let key: String = key
        .chars()
        .flat_map(char::to_lowercase)
        .filter(char::is_ascii_alphanumeric)
        .collect();
    key == "auth"
        || key == "auths"
        || [
            "password",
            "passwd",
            "passphrase",
            "secret",
            "token",
            "credential",
            "privatekey",
            "apikey",
            "authorization",
            "accesskey",
            "dockerconfig",
        ]
        .iter()
        .any(|s| key.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn dynamic_wrappers_charge_depth() {
        struct Wrapped(usize);
        impl Serialize for Wrapped {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                if self.0 == 0 {
                    return serializer.serialize_map(Some(0))?.end();
                }
                serializer.serialize_newtype_struct(MARKER, &Wrapped(self.0 - 1))
            }
        }
        assert!(super::super::project(&Wrapped(127)).is_ok());
        assert!(matches!(
            super::super::project(&Wrapped(128)),
            Err(super::super::Error::DepthLimit)
        ));
    }

    #[test]
    fn marked_subtree_redacts_variant_and_flattened_keys() {
        #[derive(Serialize)]
        enum Event {
            #[serde(rename = "token")]
            Token(&'static str),
            #[serde(rename = "token-tuple")]
            Tuple(&'static str, &'static str),
            #[serde(rename = "token-struct")]
            Struct { field: &'static str },
        }
        #[derive(Serialize)]
        #[serde(tag = "token", content = "payload")]
        enum Tagged {
            Variant(&'static str),
        }
        #[derive(Serialize)]
        struct Inner {
            token: &'static str,
            name: &'static str,
        }
        #[derive(Serialize)]
        struct Record {
            event: Event,
            tuple: Event,
            object: Event,
            tagged: Tagged,
            #[serde(flatten)]
            inner: Inner,
        }
        let record = Record {
            event: Event::Token("secret-value"),
            tuple: Event::Tuple("secret-value", "secret-value"),
            object: Event::Struct {
                field: "secret-value",
            },
            tagged: Tagged::Variant("safe"),
            inner: Inner {
                token: "secret-value",
                name: "safe",
            },
        };
        let input = DynamicMap(BTreeMap::from([("record", record)]));
        let projected = super::super::project(&input).unwrap();
        let mut output = Vec::new();
        super::super::write_json(&mut output, &projected).unwrap();
        assert!(!output.windows(12).any(|w| w == b"secret-value"));
        assert_eq!(output.windows(8).filter(|w| *w == b"redacted").count(), 5);
        assert!(output.windows(4).any(|w| w == b"safe"));
    }

    #[test]
    fn sensitive_value_is_replaced_before_serialization() {
        let map = DynamicMap(BTreeMap::from([("token", f64::NAN)]));
        let projected = super::super::project(&map).unwrap();
        let mut output = Vec::new();
        super::super::write_json(&mut output, &projected).unwrap();
        assert!(output.windows(8).any(|w| w == b"redacted"));
    }
}
