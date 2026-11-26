use crate::wire::strict_json::Field;
use serde::de::DeserializeOwned;

#[allow(dead_code)]
pub(crate) trait Output: DeserializeOwned + Default {
    const FIELDS: &'static [Field<'static>];
}

#[allow(dead_code)]
fn decode<T: Output>(body: &[u8], allow_empty: bool, cap: usize) -> Result<T, ()> {
    if allow_empty && body.is_empty() {
        return Ok(T::default());
    }
    crate::wire::strict_json::validate(body, T::FIELDS, cap).map_err(|_| ())?;
    serde_json::from_slice(body).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, serde::Deserialize, PartialEq)]
    struct Example {
        id: String,
    }

    impl Output for Example {
        const FIELDS: &'static [Field<'static>] = &[Field {
            name: "id",
            children: &[],
        }];
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(decode::<Example>(br#"{"id":"one","Id":"two"}"#, false, 64).is_err());
        assert!(decode::<Example>(br#"{"id":"one"}"#, false, 64).is_ok());
    }

    #[test]
    fn decodes_typed_response() {
        assert_eq!(
            decode::<Example>(br#"{"id":"one"}"#, false, 64),
            Ok(Example { id: "one".into() })
        );
    }
}
