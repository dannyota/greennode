use std::collections::HashSet;
use std::fmt;

use serde::Serialize;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

use super::store::{ReadOutcome, Store};

#[allow(dead_code)]
pub(crate) enum Kind {
    Iam,
    ServiceAccount,
}

#[allow(dead_code)]
pub(crate) fn read(
    store: &Store,
    name: &str,
    kind: Kind,
    now: OffsetDateTime,
    rejected: Option<&crate::Secret>,
) -> Result<Option<Entry>, Box<crate::Error>> {
    match store.read_entry(name)? {
        ReadOutcome::Missing | ReadOutcome::InvalidUtf8 => Ok(None),
        ReadOutcome::Oversize => match kind {
            Kind::Iam => Err(Box::new(crate::Error::new(crate::ErrorClass::TokenCache))),
            Kind::ServiceAccount => Ok(None),
        },
        ReadOutcome::Data(raw) => Ok(decode(&raw, now, rejected)),
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) token: crate::Secret,
    pub(crate) expires: OffsetDateTime,
    pub(crate) obtained: OffsetDateTime,
}

struct Parsed<'a> {
    token: &'a str,
    expires: Option<&'a serde_json::value::RawValue>,
    obtained: Option<&'a serde_json::value::RawValue>,
}

impl<'de> Deserialize<'de> for Parsed<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;
        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = Parsed<'de>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("cache object")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(Parsed {
                    token: "",
                    expires: None,
                    obtained: None,
                })
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut value = Parsed {
                    token: "",
                    expires: None,
                    obtained: None,
                };
                let mut seen = HashSet::new();
                let mut assigned = [false; 3];
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                    let index = ["accessToken", "expiresAt", "obtainedAt"]
                        .iter()
                        .position(|name| equal_fold(&key, name));
                    match index {
                        Some(index) => {
                            let slot = assigned
                                .get_mut(index)
                                .ok_or_else(|| serde::de::Error::custom("invalid field"))?;
                            if *slot {
                                return Err(serde::de::Error::custom("duplicate field"));
                            }
                            *slot = true;
                            match index {
                                0 => {
                                    let raw = map
                                        .next_value::<Option<&'de serde_json::value::RawValue>>()?;
                                    value.token = match raw {
                                        None => "",
                                        Some(raw) => {
                                            let text = raw.get();
                                            if !text.starts_with('"')
                                                || !text.ends_with('"')
                                                || text.as_bytes().contains(&92)
                                            {
                                                return Err(serde::de::Error::custom(
                                                    "invalid token",
                                                ));
                                            }
                                            text.get(1..text.len().saturating_sub(1)).ok_or_else(
                                                || serde::de::Error::custom("invalid token"),
                                            )?
                                        }
                                    };
                                }
                                1 => {
                                    value.expires = map
                                        .next_value::<Option<&'de serde_json::value::RawValue>>()?
                                }
                                2 => {
                                    value.obtained = map
                                        .next_value::<Option<&'de serde_json::value::RawValue>>()?
                                }
                                _ => return Err(serde::de::Error::custom("invalid field")),
                            }
                        }
                        None => {
                            map.next_value::<Strict>()?;
                        }
                    }
                }
                Ok(value)
            }
        }
        deserializer.deserialize_any(EntryVisitor)
    }
}

fn equal_fold(value: &str, expected: &str) -> bool {
    value
        .chars()
        .map(|c| match c {
            '\u{017f}' => 's',
            '\u{212a}' => 'k',
            _ => c.to_ascii_lowercase(),
        })
        .eq(expected.chars().map(|c| c.to_ascii_lowercase()))
}

struct Strict;
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON value")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Strict, E> {
                Ok(Strict)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Strict, S::Error> {
                while seq.next_element::<Strict>()?.is_some() {}
                Ok(Strict)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Strict, M::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                    map.next_value::<Strict>()?;
                }
                Ok(Strict)
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

fn timestamp(raw: &serde_json::value::RawValue) -> Option<OffsetDateTime> {
    let text = raw.get().strip_prefix('"')?.strip_suffix('"')?;
    if !text.contains(char::from(92)) {
        return OffsetDateTime::parse(text, &Rfc3339).ok();
    }
    let mut decoded = Zeroizing::new(String::with_capacity(text.len()));
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        let value = if character == char::from(92) {
            match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'b' => '\u{0008}',
                'f' => '\u{000c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let mut code = 0u32;
                    for _ in 0..4 {
                        code = code * 16 + chars.next()?.to_digit(16)?;
                    }
                    char::from_u32(code)?
                }
                _ => return None,
            }
        } else {
            character
        };
        if !value.is_ascii() {
            return None;
        }
        decoded.push(value);
    }
    OffsetDateTime::parse(&decoded, &Rfc3339).ok()
}

#[allow(dead_code)]
pub(crate) fn decode(
    raw: &crate::Secret,
    now: OffsetDateTime,
    rejected: Option<&crate::Secret>,
) -> Option<Entry> {
    let parsed: Parsed<'_> = serde_json::from_str(raw.expose_secret()).ok()?;
    let zero = OffsetDateTime::parse("0001-01-01T00:00:00Z", &Rfc3339).ok()?;
    let expires = match parsed.expires {
        Some(s) => timestamp(s)?,
        None => zero,
    };
    let obtained = match parsed.obtained {
        Some(s) => timestamp(s)?,
        None => zero,
    };
    if parsed.token.is_empty() || expires - now <= time::Duration::seconds(30) {
        return None;
    }
    if rejected.is_some_and(|value| {
        !value.expose_secret().is_empty() && value.expose_secret() == parsed.token
    }) {
        let age = now - obtained;
        if age < time::Duration::ZERO || age >= time::Duration::seconds(30) {
            return None;
        }
    }
    Some(Entry {
        token: crate::Secret::new(parsed.token),
        expires,
        obtained,
    })
}

#[derive(Serialize)]
struct Wire<'a> {
    #[serde(rename = "accessToken")]
    token: &'a str,
    #[serde(rename = "expiresAt")]
    expires: &'a str,
    #[serde(rename = "obtainedAt")]
    obtained: &'a str,
}

#[allow(dead_code)]
pub(crate) fn write(
    store: &Store,
    name: &str,
    entry: &Entry,
    context: &crate::RequestContext,
    clock: &dyn crate::Clock,
) -> Result<(), Box<crate::Error>> {
    write_with_guard(store, name, entry, || {
        context
            .check(clock)
            .map_or(Ok(()), |end| Err(Box::new(end.into())))
    })
}

fn write_with_guard(
    store: &Store,
    name: &str,
    entry: &Entry,
    guard: impl Fn() -> Result<(), Box<crate::Error>>,
) -> Result<(), Box<crate::Error>> {
    guard()?;
    let token = entry.token.expose_secret();
    let error = || Box::new(crate::Error::new(crate::ErrorClass::TokenCache));
    if token.len() > 65_536
        || token.chars().any(|c| {
            c.is_control() || matches!(c, '"' | '\\' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}')
        })
    {
        return Err(error());
    }
    let expires = Zeroizing::new(entry.expires.format(&Rfc3339).map_err(|_| error())?);
    let obtained = Zeroizing::new(entry.obtained.format(&Rfc3339).map_err(|_| error())?);
    let wire = Wire {
        token,
        expires: &expires,
        obtained: &obtained,
    };
    let mut data = Zeroizing::new(Vec::with_capacity(65_536 + 512));
    serde_json::to_writer(&mut *data, &wire).map_err(|_| error())?;
    let text = std::str::from_utf8(&data).map_err(|_| error())?;
    store.write_guarded(name, &crate::Secret::new(text), guard)
}

#[cfg(test)]
mod tests;
