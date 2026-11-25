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
    fn sensitive_value_is_replaced_before_serialization() {
        let map = DynamicMap(BTreeMap::from([("token", f64::NAN)]));
        let projected = super::super::project(&map).unwrap();
        let mut output = Vec::new();
        super::super::write_json(&mut output, &projected).unwrap();
        assert!(output.windows(8).any(|w| w == b"redacted"));
    }
}
