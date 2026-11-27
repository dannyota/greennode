use std::{borrow::Cow, collections::BTreeMap};

use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Number, Value, value::RawValue};

use crate::wire::product::evidence::Evidence;

thread_local! {
    static LENIENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn lenient_decoding() -> bool {
    LENIENT.with(std::cell::Cell::get)
}

pub(crate) fn decode_lenient<T>(decode: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LENIENT.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(LENIENT.with(|flag| flag.replace(true)));
    decode()
}

pub(crate) trait WireField: Default + DeserializeOwned {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<Self>::deserialize(d).map(Option::unwrap_or_default)
    }

    fn redact(&mut self, evidence: &Evidence) -> bool;

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        self.redact(evidence)
    }

    fn identifiers_reflect(&self, _: &Evidence) -> bool {
        false
    }
}

pub(crate) struct Field<T>(pub(crate) T);

impl<'de, T: WireField> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::read(d).map(Self)
    }
}

impl<T: WireField> WireField for Vec<T> {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let rows = Option::<Vec<Field<T>>>::deserialize(d)?.unwrap_or_default();
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.iter_mut().all(|item| item.redact(evidence))
    }

    fn identifiers_reflect(&self, evidence: &Evidence) -> bool {
        self.iter().any(|item| item.identifiers_reflect(evidence))
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        self.iter_mut().all(|item| item.redact_identifier(evidence))
    }
}

impl<T: WireField> WireField for Option<T> {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<Field<T>>::deserialize(d).map(|field| field.map(|field| field.0))
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        self.as_mut().map_or_else(
            || !evidence.text_reflects("null"),
            |item| item.redact(evidence),
        )
    }

    fn identifiers_reflect(&self, evidence: &Evidence) -> bool {
        self.as_ref()
            .is_some_and(|item| item.identifiers_reflect(evidence))
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        self.as_mut().map_or_else(
            || !evidence.text_reflects("null"),
            |item| item.redact_identifier(evidence),
        )
    }
}

impl WireField for String {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        if evidence.text_reflects(self) {
            *self = "<redacted>".into();
            return !evidence.text_reflects(self);
        }
        true
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        crate::wire::product::evidence::redact_identifier(self, evidence.credentials());
        !evidence.id_reflects(self)
    }
}

impl WireField for i64 {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match <&RawValue>::deserialize(d)?.get() {
            "null" | "-0" => Ok(0),
            value => serde_json::from_str(value).map_err(serde::de::Error::custom),
        }
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence.integer_output_reflects(*self)
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        !evidence.identifier_output_reflects(&Value::from(*self))
    }
}

impl WireField for f64 {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(d)?;
        match raw.get() {
            "null" => Ok(0.0),
            value if matches!(value.as_bytes().first(), Some(b'-' | b'0'..=b'9')) => value
                .parse::<Self>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| serde::de::Error::custom("invalid number")),
            _ => Err(serde::de::Error::custom("expected number")),
        }
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence.output_number_reflects(*self, true)
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        !evidence.identifier_output_reflects(&Value::from(*self))
    }
}

impl WireField for bool {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence
            .credentials()
            .reflects(if *self { b"true" } else { b"false" })
    }
}

impl WireField for Value {
    fn read<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(d)?;
        dynamic(raw).map_err(serde::de::Error::custom)
    }

    fn redact(&mut self, evidence: &Evidence) -> bool {
        evidence.redact(self)
    }

    fn redact_identifier(&mut self, evidence: &Evidence) -> bool {
        evidence.redact_identifier_value(self)
    }
}

fn dynamic(raw: &RawValue) -> Result<Value, serde_json::Error> {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => {
            crate::wire::strict_json::validate(text.as_bytes(), &[], text.len())
                .map_err(|_| serde::de::Error::custom("invalid JSON object"))?;
            serde_json::from_str::<BTreeMap<String, &RawValue>>(text)?
                .into_iter()
                .map(|(key, value)| Ok((key, dynamic(value)?)))
                .collect::<Result<_, _>>()
                .map(Value::Object)
        }
        Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(text)?
            .into_iter()
            .map(dynamic)
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Some(b'-' | b'0'..=b'9') => {
            let value = text.parse::<f64>().map_err(serde::de::Error::custom)?;
            Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| serde::de::Error::custom("invalid number"))
        }
        _ => serde_json::from_str(text),
    }
}

pub(crate) fn serialize_slice<T: Serialize, S: Serializer>(
    value: &Option<Vec<T>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.as_deref().unwrap_or_default().serialize(serializer)
}

pub(crate) fn serialize_dynamic<S: Serializer>(value: &Value, s: S) -> Result<S::Ok, S::Error> {
    Detail {
        value,
        marked: false,
    }
    .serialize(s)
}

struct Detail<'a> {
    value: &'a Value,
    marked: bool,
}

impl Serialize for Detail<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Object(_) if !self.marked => s.serialize_newtype_struct(
                "$greennode::dynamic-map",
                &Detail {
                    value: self.value,
                    marked: true,
                },
            ),
            Value::Object(object) => {
                let mut map = s.serialize_map(Some(object.len()))?;
                for (key, value) in object {
                    map.serialize_entry(
                        key,
                        &Detail {
                            value,
                            marked: true,
                        },
                    )?;
                }
                map.end()
            }
            Value::Array(values) => {
                let mut list = s.serialize_seq(Some(values.len()))?;
                for value in values {
                    list.serialize_element(&Detail {
                        value,
                        marked: self.marked,
                    })?;
                }
                list.end()
            }
            Value::Number(number) => number
                .as_f64()
                .ok_or_else(|| serde::ser::Error::custom("invalid number"))?
                .serialize(s),
            value => value.serialize(s),
        }
    }
}

pub(crate) fn go_json(bytes: &[u8]) -> Cow<'_, str> {
    let text = go_utf8(bytes);
    let bytes = text.as_bytes();
    let mut normalized = String::new();
    let mut start = 0;
    let mut index = 0;
    let mut in_string = false;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'"' {
            in_string = !in_string;
        }
        if !in_string || byte != b'\\' {
            index += 1;
            continue;
        }
        let Some(unit) = utf16_escape(bytes, index) else {
            index += 2;
            continue;
        };
        let end = index + 6;
        if (0xD800..=0xDBFF).contains(&unit)
            && utf16_escape(bytes, end).is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
        {
            index = end + 6;
            continue;
        }
        if (0xD800..=0xDFFF).contains(&unit) {
            if let Some(prefix) = text.get(start..index) {
                normalized.push_str(prefix);
            }
            normalized.push(char::REPLACEMENT_CHARACTER);
            start = end;
        }
        index = end;
    }
    if start == 0 {
        return text;
    }
    if let Some(rest) = text.get(start..) {
        normalized.push_str(rest);
    }
    Cow::Owned(normalized)
}

fn utf16_escape(bytes: &[u8], offset: usize) -> Option<u16> {
    let digits = bytes.get(offset..offset + 6)?.strip_prefix(b"\\u")?;
    let digits = std::str::from_utf8(digits).ok()?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

fn go_utf8(bytes: &[u8]) -> Cow<'_, str> {
    let mut chunks = bytes.utf8_chunks();
    let Some(first) = chunks.next() else {
        return Cow::Borrowed("");
    };
    if first.invalid().is_empty() {
        return Cow::Borrowed(first.valid());
    }
    let mut normalized = String::with_capacity(bytes.len());
    for chunk in std::iter::once(first).chain(chunks) {
        normalized.push_str(chunk.valid());
        normalized.extend(std::iter::repeat_n(
            char::REPLACEMENT_CHARACTER,
            chunk.invalid().len(),
        ));
    }
    Cow::Owned(normalized)
}

macro_rules! model {
    ($(@ $ignore:literal;)? $(#[$model_meta:meta])* $vis:vis struct $model:ident {
        $($(#[$field_meta:meta])* $field:ident: $type:ty => $wire:literal, $go:literal;)*
    } $(derive {
        $(#[$derived_meta:meta])* $derived:ident: $derived_type:ty => $derived_go:literal, $derive:expr;
    })?) => {
        $(#[$model_meta])*
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
        #[serde(remote = "Self")]
        $vis struct $model {
            $(
                $(#[$field_meta])*
                #[serde(rename = $go)]
                pub $field: $type,
            )*
            $(
                $(#[$derived_meta])*
                #[serde(rename = $derived_go)]
                pub $derived: $derived_type,
            )?
        }

        impl serde::Serialize for $model {
            fn serialize<S: serde::Serializer>(&self, output: S) -> Result<S::Ok, S::Error> {
                $(if $crate::wire::product::identifier::declared($wire, $go) {
                    $crate::wire::product::identifier::check::<_, S::Error>(&self.$field)?;
                })*
                Self::serialize(self, output)
            }
        }

        impl<'de> serde::Deserialize<'de> for $model {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                struct Row;

                impl<'de> serde::de::Visitor<'de> for Row {
                    type Value = $model;

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("a response object")
                    }

                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<$model, M::Error> {
                        let mut result = $model::default();
                        let mut seen = std::collections::HashSet::new();
                        let mut matched = std::collections::HashSet::new();
                        let ignore_case_variants = false $(|| $ignore)? || $crate::model::wire::lenient_decoding();
                        while let Some(key) = map.next_key::<String>()? {
                            if !seen.insert(key.clone()) {
                                return Err(serde::de::Error::custom("duplicate response field"));
                            }
                            $(
                                if $crate::wire::strict_json::field_matches($wire, &key) {
                                    if !matched.insert($wire) {
                                        return Err(serde::de::Error::custom("duplicate response field"));
                                    }
                                    if key != $wire {
                                        if !ignore_case_variants {
                                            return Err(serde::de::Error::custom("incorrect response field case"));
                                        }
                                        map.next_value::<serde::de::IgnoredAny>()?;
                                        continue;
                                    }
                                    result.$field = map.next_value::<$crate::model::wire::Field<$type>>()?.0;
                                    continue;
                                }
                            )*
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                        $(result.$derived = ($derive)(&result);)?
                        Ok(result)
                    }
                }

                decoder.deserialize_map(Row)
            }
        }

        impl $crate::model::wire::WireField for $model {
            fn redact(&mut self, evidence: &$crate::wire::product::evidence::Evidence) -> bool {
                $((if $crate::wire::product::identifier::declared($wire, $go) {
                    self.$field.redact_identifier(evidence)
                } else {
                    self.$field.redact(evidence)
                }) &&)* $(self.$derived.redact(evidence) &&)? true
            }

            fn identifiers_reflect(&self, evidence: &$crate::wire::product::evidence::Evidence) -> bool {
                $( (($crate::wire::product::identifier::declared($wire, $go))
                    && serde_json::to_value(&self.$field).map_or(true, |value| evidence.identifier_output_reflects(&value)))
                    || self.$field.identifiers_reflect(evidence) ||)* false
            }
        }
    };
}

pub(crate) use model;

#[cfg(test)]
mod lenient_tests {
    use super::{decode_lenient, lenient_decoding};

    model! {
        struct Probe {
            name: String => "name", "Name";
        }
    }

    fn lone() -> Result<Probe, serde_json::Error> {
        serde_json::from_str(r#"{"NAME":"x"}"#)
    }

    #[test]
    fn strict_decoding_refuses_a_lone_variant() {
        assert!(lone().is_err());
    }

    #[test]
    fn lenient_decoding_ignores_it_and_still_refuses_collisions() {
        assert!(decode_lenient(lone).is_ok());
        assert!(
            decode_lenient(|| serde_json::from_str::<Probe>(r#"{"name":"a","Name":"b"}"#)).is_err()
        );
    }

    #[test]
    fn strict_decoding_resumes_after_a_lenient_one() {
        assert!(decode_lenient(lone).is_ok());
        assert!(!lenient_decoding());
        assert!(lone().is_err());
    }

    #[test]
    fn nesting_keeps_lenient_until_the_outer_scope_ends() {
        decode_lenient(|| {
            decode_lenient(|| assert!(lone().is_ok()));
            assert!(lenient_decoding());
            assert!(lone().is_ok());
        });
        assert!(!lenient_decoding());
    }

    #[test]
    fn a_panic_inside_the_scope_restores_strict_decoding() {
        let result = std::panic::catch_unwind(|| decode_lenient(|| panic!("probe")));
        assert!(result.is_err());
        assert!(!lenient_decoding());
        assert!(lone().is_err());
    }
}
