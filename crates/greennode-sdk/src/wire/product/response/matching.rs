use super::{Screen, SentCredentials, automaton::SCAN_BLOCK, marker, patterns::Patterns};
use crate::go_json::{
    escaped_code_unit, hex_digit, printable_byte_prefix, repeated_code_unit_prefix,
};
use std::rc::Rc;
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
pub(super) enum MatchContext {
    ErrorText,
    Text,
    Identifier,
}

pub(super) fn matches_credential_forms(
    text: &[u8],
    credentials: &SentCredentials,
    context: MatchContext,
) -> bool {
    let mut work = matching_work_limit(text.len(), matches!(context, MatchContext::ErrorText));
    !match_with_budget(text, credentials, context, &mut work).is_clean()
}

pub(in crate::wire::product) struct BodyMatcher<'a> {
    credentials: &'a SentCredentials,
    remaining: std::cell::Cell<usize>,
}

impl<'a> BodyMatcher<'a> {
    pub(in crate::wire::product) fn new(credentials: &'a SentCredentials, len: usize) -> Self {
        Self {
            credentials,
            remaining: std::cell::Cell::new(matching_work_limit(len, false)),
        }
    }

    pub(in crate::wire::product) fn for_output(
        credentials: &'a SentCredentials,
        len: usize,
    ) -> Self {
        Self {
            credentials,
            remaining: std::cell::Cell::new(len.saturating_mul(48).clamp(8 << 20, 1 << 30)),
        }
    }

    pub(in crate::wire::product) fn with_work(
        credentials: &'a SentCredentials,
        remaining: usize,
    ) -> Self {
        Self {
            credentials,
            remaining: std::cell::Cell::new(remaining),
        }
    }

    pub(in crate::wire::product) fn remaining(&self) -> usize {
        self.remaining.get()
    }

    pub(in crate::wire::product) fn raw(&self, bytes: &[u8]) -> bool {
        if bytes.is_empty() || self.credentials.values().is_empty() {
            return false;
        }
        let Some(patterns) = self.credentials.patterns() else {
            return true;
        };
        let mut work = self.remaining.get();
        let reflects = patterns.raw(bytes, &mut work);
        self.remaining.set(work);
        reflects
    }

    pub(in crate::wire::product) fn raw_or_percent_decoded(&self, bytes: &[u8]) -> bool {
        if self.raw(bytes) {
            return true;
        }
        if self.credentials.values().is_empty() {
            return false;
        }
        let Some(remaining) = self.remaining.get().checked_sub(bytes.len()) else {
            return true;
        };
        self.remaining.set(remaining);
        match percent_decode(bytes) {
            Some(decoded) => self.raw(&decoded),
            None => self.raw(bytes),
        }
    }

    pub(in crate::wire::product) fn reflects(&self, bytes: &[u8], identifier: bool) -> bool {
        !self.scan(bytes, identifier).is_clean()
    }

    pub(in crate::wire::product) fn scan(&self, bytes: &[u8], identifier: bool) -> Screen {
        if self.credentials.values().is_empty() {
            return Screen::Clean;
        }
        let context = if identifier {
            MatchContext::Identifier
        } else {
            MatchContext::Text
        };
        let mut work = self.remaining.get();
        let reflects = match_with_budget(bytes, self.credentials, context, &mut work);
        self.remaining.set(work);
        reflects
    }
}

fn match_with_budget(
    text: &[u8],
    credentials: &SentCredentials,
    context: MatchContext,
    work: &mut usize,
) -> Screen {
    let Some(patterns) = credentials.patterns() else {
        return Screen::Indeterminate;
    };
    let error_text = matches!(context, MatchContext::ErrorText);
    let mut state = MatchState {
        work: *work,
        forms: Vec::new(),
        visited: Vec::new(),
        patterns,
        error_text,
        strict_base64: matches!(context, MatchContext::Identifier),
    };
    let reflects = matches_forms(MatchInput::Borrowed(text), [0; 4], &mut state);
    *work = state.work;
    reflects
}

pub(super) fn matching_work_limit(length: usize, error_text: bool) -> usize {
    const LIMIT: usize = 8 << 20;
    if error_text {
        LIMIT
    } else {
        length.saturating_mul(64).clamp(LIMIT, 24 * LIMIT)
    }
}

#[derive(Clone)]
enum MatchInput<'a> {
    Borrowed(&'a [u8]),
    Decoded(Rc<Zeroizing<Vec<u8>>>),
}

impl MatchInput<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Decoded(bytes) => bytes,
        }
    }
}

struct MatchState<'a, 'text> {
    work: usize,
    forms: Vec<(MatchInput<'text>, bool)>,
    visited: Vec<(usize, [u8; 4])>,
    patterns: &'a Patterns,
    error_text: bool,
    strict_base64: bool,
}

impl MatchState<'_, '_> {
    fn charge(&mut self, work: usize) -> bool {
        if work >= self.work {
            return false;
        }
        self.work -= work;
        true
    }
}

fn matches_forms<'text>(
    form: MatchInput<'text>,
    counts: [u8; 4],
    state: &mut MatchState<'_, 'text>,
) -> Screen {
    let input = form.bytes();
    let decoded_base64 = counts[3] != 0;
    let mut found = None;
    for index in 0..state.forms.len() {
        if !state.charge(input.len()) {
            return Screen::Indeterminate;
        }
        if state.forms[index].0.bytes() == input && state.forms[index].1 == decoded_base64 {
            found = Some(index);
            break;
        }
    }
    let index = if let Some(index) = found {
        index
    } else {
        if !state.charge(input.len()) {
            return Screen::Indeterminate;
        }
        if state.error_text
            && (marker::has_credential_marker_bytes(input)
                || (!state.patterns.is_empty() && std::str::from_utf8(input).is_err()))
        {
            return Screen::Reflected;
        }
        let screen = Screen::from_match(state.patterns.forms(
            input,
            decoded_base64,
            state.strict_base64,
            &mut state.work,
        ));
        if !screen.is_clean() {
            return screen;
        }
        state.forms.push((form.clone(), decoded_base64));
        state.forms.len() - 1
    };
    if state.visited.contains(&(index, counts)) {
        return Screen::Clean;
    }
    state.visited.push((index, counts));
    for step in 0..4 {
        if !state.charge(input.len()) {
            return Screen::Indeterminate;
        }
        let next = match step {
            0 => remove_controls(input),
            1 => json_unescape(input),
            2 => percent_decode(input),
            _ if state
                .patterns
                .can_decode_base64(input.len(), state.strict_base64) =>
            {
                decode_base64(input)
            }
            _ => None,
        };
        if let Some(next) = next {
            if step != 3 && counts[step] == 2 {
                return Screen::Indeterminate;
            }
            let mut next_counts = counts;
            next_counts[step] = if step == 3 { 1 } else { counts[step] + 1 };
            let error_text = state.error_text;
            if step == 3 {
                state.error_text = false;
            }
            let matched = matches_forms(MatchInput::Decoded(Rc::new(next)), next_counts, state);
            state.error_text = error_text;
            if !matched.is_clean() {
                return matched;
            }
        }
    }
    Screen::Clean
}

fn remove_controls(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let first = printable_byte_prefix(input);
    if first == input.len() {
        return None;
    }
    let mut output = Zeroizing::new(Vec::with_capacity(input.len()));
    output.extend_from_slice(&input[..first]);
    let mut previous = None;
    for block in input[first + 1..].chunks(SCAN_BLOCK) {
        if let Some((input_block, start, end)) = previous
            && block == input_block
        {
            output.extend_from_within(start..end);
        } else {
            let start = output.len();
            let mut rest = block;
            loop {
                let end = printable_byte_prefix(rest);
                output.extend_from_slice(&rest[..end]);
                if end == rest.len() {
                    break;
                }
                rest = &rest[end + 1..];
            }
            previous = Some((block, start, output.len()));
        }
    }
    Some(output)
}

fn decode_base64(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if input.is_empty() || !input.is_ascii() {
        return None;
    }
    let url_safe = input.contains(&b'-') || input.contains(&b'_');
    let padded = input.contains(&b'=');
    let encoding = match (url_safe, padded) {
        (false, false) => &data_encoding::BASE64_NOPAD,
        (false, true) => &data_encoding::BASE64,
        (true, false) => &data_encoding::BASE64URL_NOPAD,
        (true, true) => &data_encoding::BASE64URL,
    };
    let len = encoding.decode_len(input.len()).ok()?;
    let tail = (input.len().saturating_sub(1) / 4) * 4;
    let mut last = Zeroizing::new([0; 3]);
    let tail_len = encoding.decode_len(input.len() - tail).ok()?;
    encoding
        .decode_mut(&input[tail..], &mut last[..tail_len])
        .ok()?;
    let mut previous = &[][..];
    for block in input[..tail].chunks(SCAN_BLOCK) {
        if block != previous {
            if !block.iter().copied().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || (padded && byte == b'=')
                    || if url_safe {
                        matches!(byte, b'-' | b'_')
                    } else {
                        matches!(byte, b'+' | b'/')
                    }
            }) {
                return None;
            }
            previous = block;
        }
    }
    let mut decoded = Zeroizing::new(vec![0; len]);
    let mut previous = None;
    let mut written = 0;
    for block in input.chunks(SCAN_BLOCK) {
        if let Some((previous_block, start, end)) = previous
            && block == previous_block
        {
            decoded.copy_within(start..end, written);
            written += end - start;
        } else {
            let end = written + encoding.decode_len(block.len()).ok()?;
            let count = encoding
                .decode_mut(block, &mut decoded[written..end])
                .ok()?;
            previous = Some((block, written, written + count));
            written += count;
        }
    }
    decoded.truncate(written);
    Some(decoded)
}

fn json_escape(input: &[u8]) -> (Option<char>, usize) {
    let decoded = match input.get(1) {
        Some(b'"') => '"',
        Some(b'\\') => '\\',
        Some(b'/') => '/',
        Some(b'b') => '\x08',
        Some(b'f') => '\x0c',
        Some(b'n') => '\n',
        Some(b'r') => '\r',
        Some(b't') => '\t',
        Some(b'u') => {
            let Some(high) = escaped_code_unit(input) else {
                return (None, 1);
            };
            if (0xd800..=0xdbff).contains(&high) {
                let Some(low) =
                    escaped_code_unit(&input[6..]).filter(|low| (0xdc00..=0xdfff).contains(low))
                else {
                    return (None, 6);
                };
                let point = 0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(low) - 0xdc00;
                return (char::from_u32(point), 12);
            }
            return (char::from_u32(u32::from(high)), 6);
        }
        _ => return (None, 1),
    };
    (Some(decoded), 2)
}

fn json_unescape(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if !input.contains(&b'\\') {
        return None;
    }
    let mut output = None;
    let mut copied = 0;
    let mut index = 0;
    while index < input.len() {
        if input[index] != b'\\' {
            index += 1;
            continue;
        }
        let (decoded, mut size) = json_escape(&input[index..]);
        if decoded.is_none() && size == 6 {
            let repeated = repeated_code_unit_prefix(&input[index..]);
            size = (repeated - size).max(size);
        }
        if let Some(ch) = decoded {
            let buffer =
                output.get_or_insert_with(|| Zeroizing::new(Vec::with_capacity(input.len())));
            buffer.extend_from_slice(&input[copied..index]);
            buffer.extend_from_slice(ch.encode_utf8(Zeroizing::new([0; 4]).as_mut()).as_bytes());
            copied = index + size;
        }
        index += size;
    }
    let mut output = output?;
    output.extend_from_slice(&input[copied..]);
    Some(output)
}

fn percent_decode(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if !input.contains(&b'%') {
        return None;
    }
    let mut output = None;
    let mut copied = 0;
    let mut index = 0;
    while index + 2 < input.len() {
        if input[index] == b'%'
            && let (Some(a), Some(b)) = (hex_digit(input[index + 1]), hex_digit(input[index + 2]))
        {
            let buffer =
                output.get_or_insert_with(|| Zeroizing::new(Vec::with_capacity(input.len())));
            buffer.extend_from_slice(&input[copied..index]);
            buffer.push((a << 4) | b);
            index += 3;
            copied = index;
        } else {
            index += 1;
        }
    }
    let mut output = output?;
    output.extend_from_slice(&input[copied..]);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_fast_path_preserves_all_alphabets_and_concatenated_padding() {
        for input in [
            "YQ==Yg==", "YQ==YmNk", "YWI", "//8=", "__8", "Yx", "Ya==", "YQ-_+/",
        ] {
            let old = [
                data_encoding::BASE64,
                data_encoding::BASE64_NOPAD,
                data_encoding::BASE64URL,
                data_encoding::BASE64URL_NOPAD,
            ]
            .iter()
            .find_map(|encoding| encoding.decode(input.as_bytes()).ok());
            let decoded = decode_base64(input.as_bytes());
            assert_eq!(decoded.as_deref(), old.as_ref());
        }
    }

    #[test]
    fn repeated_transforms_preserve_complete_output() {
        let input = "YQ==YmNk".repeat(SCAN_BLOCK / 4);
        let expected = data_encoding::BASE64.decode(input.as_bytes()).unwrap();
        assert!(decode_base64(input.as_bytes()).as_deref() == Some(&expected));
        let filtered = b"\x1cxy".repeat(SCAN_BLOCK);
        assert!(remove_controls(&filtered).as_deref() == Some(&b"xy".repeat(SCAN_BLOCK)));
    }

    #[test]
    fn json_escape_fast_path_preserves_mixed_and_unpaired_escapes() {
        for (wire, expected) in [
            (br"plain".as_slice(), None),
            (br"\uD800\uD801", None),
            (br"\u0061\uD800\u0062", Some(br"a\uD800b".as_slice())),
            (br"\uD83D\uDE00\n\u0062", Some("😀\nb".as_bytes())),
        ] {
            let decoded = json_unescape(wire);
            assert_eq!(decoded.as_ref().map(|text| text.as_slice()), expected);
        }
    }

    #[test]
    fn ten_long_credentials_allow_clean_256_kib_output() {
        let credentials = SentCredentials::for_test(
            (0..10)
                .map(|index| {
                    crate::Secret::new(format!(
                        "synthetic-credential-{index}-{}",
                        "secret".repeat(12)
                    ))
                })
                .collect(),
        );
        let output = "ordinary output!".repeat(1 << 14);
        assert_eq!(output.len(), 256 << 10);
        assert!(
            !BodyMatcher::for_output(&credentials, output.len()).reflects(output.as_bytes(), false)
        );
        let evidence = crate::wire::product::evidence::Evidence::from_credentials(&credentials);
        assert!(!evidence.serialized_output_reflects(&serde_json::json!({"output": output})));
    }

    #[test]
    fn bounded_output_scan_refuses_an_adversarial_prefix_run() {
        let credentials = super::super::budget_test_credentials();
        let mut output = vec![b'a'; 1 << 16];
        output[0] = b'!';
        let budget = 8 * output.len();
        let clean = vec![b'!'; output.len()];
        let matcher = BodyMatcher::with_work(&credentials, budget);
        assert!(!matcher.reflects(&clean, false));
        assert!(matcher.remaining() > 0);
        let matcher = BodyMatcher::with_work(&credentials, budget);
        assert!(matcher.reflects(&output, false));
        assert_eq!(matcher.remaining(), 0);
        let matcher = BodyMatcher::with_work(&credentials, 64 * output.len());
        assert!(!matcher.reflects(&output, false));
    }
}
