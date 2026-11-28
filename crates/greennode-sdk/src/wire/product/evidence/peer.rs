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

pub(super) fn not_found_body_screen(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    if body
        .trim_ascii()
        .eq_ignore_ascii_case(b"404 page not found")
    {
        return Screen::CleanNotFound;
    }
    stream_screen(matcher, body, false, true, decoded_reflects).then(|| Screen::CleanNotFound)
}

pub(super) fn discovery_stream_screen(
    matcher: &BodyMatcher<'_>,
    body: &[u8],
    require_json: bool,
) -> Screen {
    stream_screen(matcher, body, require_json, false, discovery_value_screen)
}

fn stream_screen(
    matcher: &BodyMatcher<'_>,
    body: &[u8],
    require_json: bool,
    strict_after_value: bool,
    screen_value: impl Fn(&BodyMatcher<'_>, &[u8]) -> Screen,
) -> Screen {
    let text = crate::model::wire::go_json(stream_start(body, require_json));
    screen_normalized_stream(
        matcher,
        text.as_bytes(),
        require_json,
        strict_after_value,
        screen_value,
    )
}

pub(super) fn discovery_normalized_stream_screen(
    matcher: &BodyMatcher<'_>,
    normalized: &str,
    require_json: bool,
) -> Screen {
    let body = stream_start(normalized.as_bytes(), require_json);
    screen_normalized_stream(matcher, body, require_json, false, discovery_value_screen)
}

impl super::Evidence {
    pub(in crate::wire::product) fn normalized_discovery_body_screen(
        &self,
        body: &[u8],
        normalized: &str,
        accepted: bool,
    ) -> Screen {
        let matcher = BodyMatcher::new(&self.credentials, body.len());
        matcher
            .scan(body, false)
            .then(|| discovery_normalized_stream_screen(&matcher, normalized, accepted))
    }
}

fn stream_start(body: &[u8], require_json: bool) -> &[u8] {
    if require_json {
        body
    } else {
        let trimmed = body.trim_ascii_start();
        trimmed.strip_prefix(b"\xef\xbb\xbf").unwrap_or(trimmed)
    }
}

fn screen_normalized_stream(
    matcher: &BodyMatcher<'_>,
    mut rest: &[u8],
    mut require_json: bool,
    strict_after_value: bool,
    screen_value: impl Fn(&BodyMatcher<'_>, &[u8]) -> Screen,
) -> Screen {
    for value in crate::go_json::stream_values(rest) {
        rest = rest.trim_ascii_start();
        require_json |= matches!(rest.first(), Some(b'{' | b'[' | b'"'));
        let value = match value {
            Ok(value) => value,
            Err(()) if require_json => return Screen::Indeterminate,
            Err(()) if strict_after_value => return matcher.scan(rest, false),
            Err(()) => return Screen::Clean,
        };
        require_json |= strict_after_value;
        let screen = screen_value(matcher, value);
        if !screen.is_clean() {
            return screen;
        }
        rest = &rest[value.len()..];
    }
    Screen::Clean
}

fn discovery_value_screen(matcher: &BodyMatcher<'_>, bytes: &[u8]) -> Screen {
    let mut frames: Vec<(bool, bool)> = Vec::new();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let identifier = frames.last().is_some_and(|frame| frame.1);
        match byte {
            b'{' | b'[' => frames.push((identifier, identifier)),
            b'}' | b']' => {
                frames.pop();
            }
            b',' => {
                if let Some(frame) = frames.last_mut() {
                    frame.1 = frame.0;
                }
            }
            b'"' => {
                let start = index;
                index += 1;
                loop {
                    index += crate::go_json::ordinary_string_prefix(&bytes[index..]);
                    match bytes.get(index) {
                        Some(b'"') => break,
                        Some(b'\\') if index + 1 < bytes.len() => index += 2,
                        _ => return Screen::Indeterminate,
                    }
                }
                let contents = &bytes[start + 1..index];
                let decoded;
                let text = if contents.contains(&b'\\') {
                    let Ok(value) = serde_json::from_slice::<String>(&bytes[start..=index]) else {
                        return Screen::Indeterminate;
                    };
                    decoded = Zeroizing::new(value);
                    decoded.as_bytes()
                } else {
                    contents
                };
                let screen = matcher.scan(text, identifier);
                if !screen.is_clean() {
                    return screen;
                }
                if bytes[index + 1..].trim_ascii_start().first() == Some(&b':')
                    && let Some(frame) = frames.last_mut()
                {
                    frame.1 = frame.0
                        || std::str::from_utf8(text)
                            .is_ok_and(super::super::identifier::declared_wire);
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while bytes.get(index + 1).is_some_and(|byte| {
                    matches!(byte, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                }) {
                    index += 1;
                }
                let screen = matcher.scan(&bytes[start..=index], identifier);
                if !screen.is_clean() {
                    return screen;
                }
            }
            _ => {}
        }
        index += 1;
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
        .then(|| check_output(matcher, body, false, false))
}

pub(super) fn output_identifier_reflects(matcher: &BodyMatcher<'_>, body: &[u8]) -> Screen {
    check_output(matcher, body, true, false)
}

pub(in crate::wire::product) fn raw_output_with_matcher(
    matcher: &BodyMatcher<'_>,
    body: &[u8],
    identifiers: bool,
) -> Screen {
    matcher
        .scan(body, identifiers)
        .then(|| check_output(matcher, body, identifiers, true))
}

fn check_output(
    matcher: &BodyMatcher<'_>,
    body: &[u8],
    identifiers: bool,
    raw_numbers: bool,
) -> Screen {
    let text = crate::model::wire::go_json(body);
    check_decoded(
        text.as_bytes(),
        identifiers,
        identifier_key,
        &|text, identifier, number| {
            matcher.scan(text.as_bytes(), identifier).then(|| {
                if number {
                    output_number_reflects(matcher, text, identifier, raw_numbers)
                } else {
                    Screen::Clean
                }
            })
        },
    )
}

fn output_number_reflects(
    matcher: &BodyMatcher<'_>,
    text: &str,
    identifier: bool,
    raw_numbers: bool,
) -> Screen {
    let Some(number) = text.parse::<f64>().ok().filter(|number| number.is_finite()) else {
        return Screen::Clean;
    };
    if !raw_numbers
        && !identifier
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
                if let Ok(decoded) = serde_json::from_str::<&str>(raw) {
                    Ok((self.matches)(decoded, self.identifier, false))
                } else {
                    let decoded = Zeroizing::new(serde_json::from_str::<String>(raw)?);
                    Ok((self.matches)(&decoded, self.identifier, false))
                }
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
