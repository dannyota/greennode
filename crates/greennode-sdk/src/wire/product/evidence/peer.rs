use super::{
    identifier_key,
    response::{BodyMatcher, Screen},
};
use serde::Deserializer;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;
use zeroize::Zeroizing;

pub(super) fn decoded_reflects(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    serde_json::from_slice::<&RawValue>(body).map_or(Screen::Indeterminate, |value| {
        decoded_value_screen(matcher, value)
    })
}

pub(super) fn discovery_stream_screen(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    let leading = body.trim_ascii_start();
    let leading = leading
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(leading)
        .trim_ascii_start();
    let mut require_json = matches!(leading.first(), Some(b'{' | b'[' | b'"'));
    let text = crate::model::wire::go_json(body);
    let values = serde_json::Deserializer::from_str(&text).into_iter::<&RawValue>();
    for value in values {
        let Ok(value) = value else {
            return if require_json {
                Screen::Indeterminate
            } else {
                Screen::Clean
            };
        };
        let screen = decoded_value_screen(matcher, value);
        if !screen.is_clean() {
            return screen;
        }
        require_json = true;
    }
    Screen::Clean
}

fn decoded_value_screen(matcher: &BodyMatcher<'_>, value: &RawValue) -> Screen {
    check_value(
        value,
        false,
        super::super::identifier::declared_wire,
        &|text, identifier, _| matcher.scan(text.as_bytes(), identifier),
    )
}

pub(super) fn request_identifier_key(key: &str) -> bool {
    identifier_key(key) || matches!(key, "policyIdList" | "serverIdList")
}

pub(super) fn request_reflects(matcher: &BodyMatcher<'_>, body: &[u8], identifier: bool) -> bool {
    !check_decoded(
        body,
        identifier,
        request_identifier_key,
        &|text, identifier, _| matcher.scan(text.as_bytes(), identifier),
    )
    .is_clean()
}

pub(super) fn output_reflects(evidence: &super::Evidence, body: &[u8]) -> Screen {
    let matcher = BodyMatcher::for_output(evidence.credentials(), body.len());
    output_with_matcher(&matcher, body)
}

pub(super) fn output_with_matcher(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    matcher
        .scan(body, false)
        .then(|| check_output(matcher, body, false))
}

pub(super) fn output_identifier_reflects(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    check_output(matcher, body, true)
}

fn check_output(matcher: &BodyMatcher<'_>, body: &[u8], identifiers: bool) -> Screen {
    check_decoded(
        body,
        identifiers,
        identifier_key,
        &|text, identifier, number| {
            matcher.scan(text.as_bytes(), identifier).then(|| {
                if number {
                    output_number_reflects(matcher, text, identifier)
                } else {
                    Screen::Clean
                }
            })
        },
    )
}

fn output_number_reflects(matcher: &BodyMatcher<'_>, text: &str, identifier: bool) -> Screen {
    let Ok(number) = text.parse::<f64>() else {
        return Screen::Clean;
    };
    if !identifier
        && text.parse::<i64>().is_err()
        && text.parse::<u64>().is_err()
        && serde_json::to_string(&number).is_ok_and(|canonical| canonical != text)
    {
        return Screen::Clean;
    }
    let screen = std::cell::Cell::new(Screen::Clean);
    if crate::output_number::any_spelling(number, false, |spelling| {
        let matched = matcher.scan(spelling.as_bytes(), identifier);
        screen.set(matched);
        !matched.is_clean()
    }) && screen.get().is_clean()
    {
        return Screen::Indeterminate;
    }
    screen.get()
}

fn check_decoded<F: Fn(&str, bool, bool) -> Screen>(
    body: &[u8],
    root_identifier: bool,
    identifier_key: fn(&str) -> bool,
    matches: &F,
) -> Screen {
    serde_json::from_slice::<&RawValue>(body).map_or(Screen::Indeterminate, |value| {
        check_value(value, root_identifier, identifier_key, matches)
    })
}

fn check_value<F: Fn(&str, bool, bool) -> Screen>(
    value: &RawValue,
    root_identifier: bool,
    identifier_key: fn(&str) -> bool,
    matches: &F,
) -> Screen {
    Walker {
        matches,
        identifier_key,
        identifier: root_identifier,
        depth: 0,
    }
    .check(value)
    .unwrap_or(Screen::Indeterminate)
}

struct Walker<'a, F> {
    matches: &'a F,
    identifier_key: fn(&str) -> bool,
    identifier: bool,
    depth: usize,
}

impl<F: Fn(&str, bool, bool) -> Screen> Walker<'_, F> {
    fn check(self, value: &RawValue) -> Result<Screen, serde_json::Error> {
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

impl<'de, F: Fn(&str, bool, bool) -> Screen> Visitor<'de> for Walker<'_, F> {
    type Value = Screen;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object or array")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Screen, M::Error> {
        let mut screen = Screen::Clean;
        while let Some((key, raw)) = map.next_entry::<String, &RawValue>()? {
            let key = Zeroizing::new(key);
            screen = screen
                .then(|| (self.matches)(&key, self.identifier, false))
                .then(|| {
                    self.child(self.identifier || (self.identifier_key)(&key))
                        .check(raw)
                        .unwrap_or(Screen::Indeterminate)
                });
        }
        Ok(screen)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Screen, A::Error> {
        let mut screen = Screen::Clean;
        while let Some(raw) = seq.next_element::<&RawValue>()? {
            screen = screen.then(|| {
                self.child(self.identifier)
                    .check(raw)
                    .unwrap_or(Screen::Indeterminate)
            });
        }
        Ok(screen)
    }
}
