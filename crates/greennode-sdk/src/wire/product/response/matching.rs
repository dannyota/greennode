use super::{
    SentCredentials,
    encoded::{self, EncodedNeedle},
    marker,
};
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
    match_with_budget(text, credentials, context, &mut work)
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

    pub(in crate::wire::product) fn literal(&self, text: &str) -> bool {
        let Some(patterns) = self.credentials.patterns() else {
            return true;
        };
        let text = Zeroizing::new(text.to_ascii_lowercase());
        for needle in &patterns.literal {
            let Some(work) = self
                .remaining
                .get()
                .checked_sub(text.len().saturating_add(needle.len()))
            else {
                return true;
            };
            self.remaining.set(work);
            let Ok(needle) = std::str::from_utf8(needle) else {
                return true;
            };
            if text.contains(needle) {
                return true;
            }
        }
        false
    }

    pub(in crate::wire::product) fn reflects(&self, bytes: &[u8], identifier: bool) -> bool {
        if self.credentials.values().is_empty() {
            return false;
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
) -> bool {
    let Some(patterns) = credentials.patterns() else {
        return true;
    };
    let error_text = matches!(context, MatchContext::ErrorText);
    let mut state = MatchState {
        work: *work,
        forms: Vec::new(),
        visited: Vec::new(),
        encoded_needles: &patterns.base64,
        hex_needles: &patterns.hex,
        error_text,
        strict_base64: matches!(context, MatchContext::Identifier),
    };
    let reflects = matches_forms(text, &patterns.literal, [0; 4], &mut state);
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

pub(super) struct MatchState<'a> {
    pub(super) work: usize,
    pub(super) forms: Vec<(Zeroizing<Vec<u8>>, bool)>,
    pub(super) visited: Vec<(usize, [u8; 4])>,
    pub(super) encoded_needles: &'a [EncodedNeedle],
    pub(super) hex_needles: &'a [Zeroizing<String>],
    pub(super) error_text: bool,
    pub(super) strict_base64: bool,
}

impl MatchState<'_> {
    fn charge(&mut self, work: usize) -> bool {
        if work >= self.work {
            return false;
        }
        self.work -= work;
        true
    }
}

pub(super) fn matches_forms(
    input: &[u8],
    needles: &[Zeroizing<Vec<u8>>],
    counts: [u8; 4],
    state: &mut MatchState<'_>,
) -> bool {
    let decoded_base64 = counts[3] != 0;
    let mut found = None;
    for index in 0..state.forms.len() {
        if !state.charge(input.len()) {
            return true;
        }
        if state.forms[index].0.as_slice() == input && state.forms[index].1 == decoded_base64 {
            found = Some(index);
            break;
        }
    }
    let index = if let Some(index) = found {
        index
    } else {
        if !state.charge(input.len()) {
            return true;
        }
        for index in 0..state.encoded_needles.len() {
            if !state.charge(
                input
                    .len()
                    .saturating_add(state.encoded_needles[index].len()),
            ) {
                return true;
            }
            if state.encoded_needles[index].matches(input, state.strict_base64) {
                return true;
            }
        }
        let lower = Zeroizing::new(input.to_ascii_lowercase());
        for index in 0..state.hex_needles.len() {
            if !state.charge(lower.len().saturating_add(state.hex_needles[index].len()))
                || encoded::contains_hex(&lower, &state.hex_needles[index], state.strict_base64)
            {
                return true;
            }
        }
        if state.error_text && marker::has_credential_marker_bytes(input) {
            return true;
        }
        if !needles.is_empty() {
            if state.error_text && std::str::from_utf8(&lower).is_err() {
                return true;
            }
            for needle in needles {
                if decoded_base64 && !search_decoded(needle, state.strict_base64) {
                    continue;
                }
                if !state.charge(lower.len().saturating_add(needle.len())) {
                    return true;
                }
                let Ok(needle) = std::str::from_utf8(needle) else {
                    return true;
                };
                if lower
                    .utf8_chunks()
                    .any(|chunk| chunk.valid().contains(needle))
                {
                    return true;
                }
            }
        }
        state
            .forms
            .push((Zeroizing::new(input.to_vec()), decoded_base64));
        state.forms.len() - 1
    };
    if state.visited.contains(&(index, counts)) {
        return false;
    }
    state.visited.push((index, counts));
    for step in 0..4 {
        if !state.charge(input.len()) {
            return true;
        }
        let next = match step {
            0 => Zeroizing::new(
                input
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_control())
                    .collect(),
            ),
            1 => json_unescape(input),
            2 => Zeroizing::new(percent_decode(input)),
            _ if needles
                .iter()
                .map(|needle| needle.len().saturating_mul(8).saturating_add(5) / 6)
                .filter(|length| state.strict_base64 || *length >= 7)
                .min()
                .is_some_and(|minimum| input.len() >= minimum) =>
            {
                decode_base64(input).unwrap_or_else(|| Zeroizing::new(input.to_vec()))
            }
            _ => Zeroizing::new(input.to_vec()),
        };
        if next.as_slice() != input {
            if counts[step] == 2 {
                if step == 3 {
                    let lower = Zeroizing::new(next.to_ascii_lowercase());
                    for needle in needles {
                        if !search_decoded(needle, state.strict_base64) {
                            continue;
                        }
                        if !state.charge(lower.len().saturating_add(needle.len())) {
                            return true;
                        }
                        let Ok(needle) = std::str::from_utf8(needle) else {
                            return true;
                        };
                        if lower
                            .utf8_chunks()
                            .any(|chunk| chunk.valid().contains(needle))
                        {
                            return true;
                        }
                    }
                    continue;
                }
                return true;
            }
            let mut next_counts = counts;
            next_counts[step] += 1;
            let error_text = state.error_text;
            if step == 3 {
                state.error_text = false;
            }
            let matched = matches_forms(&next, needles, next_counts, state);
            state.error_text = error_text;
            if matched {
                return true;
            }
        }
    }
    false
}

fn search_decoded(needle: &[u8], identifier: bool) -> bool {
    identifier || needle.len().saturating_mul(8).div_ceil(6) >= 8
}

fn decode_base64(input: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    [
        data_encoding::BASE64,
        data_encoding::BASE64_NOPAD,
        data_encoding::BASE64URL,
        data_encoding::BASE64URL_NOPAD,
    ]
    .iter()
    .find_map(|encoding| {
        let len = encoding.decode_len(input.len()).ok()?;
        let mut decoded = Zeroizing::new(vec![0; len]);
        let written = encoding.decode_mut(input, &mut decoded).ok()?;
        decoded.truncate(written);
        Some(decoded)
    })
}

fn json_unescape(input: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut unescaped = Zeroizing::new(Vec::with_capacity(input.len()));
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'\\'
            && let Some(&next) = input.get(index + 1)
        {
            if next == b'u' && index + 6 <= input.len() {
                let mut end = index + 6;
                if end + 6 <= input.len() && &input[end..end + 2] == b"\\u" {
                    end += 6;
                }
                let mut quoted = Zeroizing::new(Vec::with_capacity(end - index + 2));
                quoted.push(b'"');
                quoted.extend_from_slice(&input[index..end]);
                quoted.push(b'"');
                if let Ok(value) = serde_json::from_slice::<String>(&quoted) {
                    let value = Zeroizing::new(value);
                    unescaped.extend_from_slice(value.as_bytes());
                    index = end;
                    continue;
                }
                if end != index + 6 {
                    quoted.truncate(1);
                    quoted.extend_from_slice(&input[index..index + 6]);
                    quoted.push(b'"');
                    if let Ok(value) = serde_json::from_slice::<String>(&quoted) {
                        let value = Zeroizing::new(value);
                        unescaped.extend_from_slice(value.as_bytes());
                        index += 6;
                        continue;
                    }
                }
            }
            let decoded = match next {
                b'"' => Some(b'"'),
                b'\\' => Some(b'\\'),
                b'/' => Some(b'/'),
                b'b' => Some(8),
                b'f' => Some(12),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                _ => None,
            };
            if let Some(byte) = decoded {
                unescaped.push(byte);
                index += 2;
                continue;
            }
        }
        unescaped.push(input[index]);
        index += 1;
    }
    unescaped
}

fn percent_decode(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == b'%'
            && index + 2 < data.len()
            && let (Some(a), Some(b)) = (
                (data[index + 1] as char).to_digit(16),
                (data[index + 2] as char).to_digit(16),
            )
        {
            output.push(((a << 4) | b) as u8);
            index += 3;
        } else {
            output.push(data[index]);
            index += 1;
        }
    }
    output
}
