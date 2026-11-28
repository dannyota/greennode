use super::{
    automaton::{Automaton, SCAN_BLOCK, block_excludes_starts, charge},
    encoded::{self, EncodedNeedle},
};
use zeroize::Zeroizing;

const LITERAL: u8 = 1;
const DECODED_LITERAL: u8 = 1 << 1;
const MAX_COMPILED_BYTES: usize = 4 << 20;

pub(in crate::wire::product) struct Patterns {
    folded: Automaton,
    base64: Automaton,
    raw: Automaton,
    minimum_base64: [Option<usize>; 2],
}

struct PatternBudget {
    remaining_parts: usize,
    remaining_bytes: usize,
}

impl PatternBudget {
    fn charge(&mut self, len: usize) -> Option<()> {
        charge(&mut self.remaining_parts, 1)?;
        charge(&mut self.remaining_bytes, len)
    }
}

impl Patterns {
    pub(in crate::wire::product) fn compile(secrets: &[crate::Secret]) -> Option<Self> {
        let mut budget = PatternBudget {
            remaining_parts: 512,
            remaining_bytes: 4 << 20,
        };
        let mut needles = Vec::new();
        let mut encoded_needles = Vec::new();
        let mut hex_needles = Vec::new();
        for secret in secrets {
            let value = secret.expose_secret();
            budget.charge(value.len())?;
            if !value.is_empty() && value.len() < 6 {
                for (encoding, url_safe) in [
                    (data_encoding::BASE64, false),
                    (data_encoding::BASE64_NOPAD, false),
                    (data_encoding::BASE64URL, true),
                    (data_encoding::BASE64URL_NOPAD, true),
                ] {
                    encoded_needles.push(EncodedNeedle::short(
                        Zeroizing::new(encoding.encode(value.as_bytes())),
                        url_safe,
                    ));
                }
            }
            for part in std::iter::once(value).chain(
                value
                    .lines()
                    .map(str::trim)
                    .filter(|line| line.len() >= 8 && *line != value),
            ) {
                if part.is_empty() {
                    continue;
                }
                budget.charge(part.len())?;
                for offset in 0..3 {
                    let mut aligned = Zeroizing::new(vec![0; offset]);
                    aligned.extend_from_slice(part.as_bytes());
                    let start = (offset * 8).div_ceil(6);
                    let end = offset.saturating_add(part.len()).saturating_mul(8) / 6;
                    if end.saturating_sub(start) >= encoded::STABLE_FLOOR {
                        for encoding in [data_encoding::BASE64, data_encoding::BASE64URL] {
                            let encoded = Zeroizing::new(encoding.encode(&aligned));
                            if let Some(stable) = encoded.get(start..end) {
                                encoded_needles.push(EncodedNeedle::fragment(Zeroizing::new(
                                    stable.to_owned(),
                                )));
                            }
                        }
                    }
                }
                hex_needles.push(Zeroizing::new(
                    data_encoding::HEXLOWER.encode(part.as_bytes()),
                ));
                needles.push(Zeroizing::new(part.as_bytes().to_ascii_lowercase()));
            }
        }
        needles.sort_unstable_by(|a, b| a.as_slice().cmp(b.as_slice()));
        needles.dedup();
        EncodedNeedle::dedup(&mut encoded_needles);
        hex_needles.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        hex_needles.dedup();
        let base64_lengths = needles
            .iter()
            .map(|needle| needle.len().saturating_mul(8).div_ceil(6));
        let minimum_base64 = [
            base64_lengths.clone().filter(|length| *length >= 7).min(),
            base64_lengths.min(),
        ];
        let literal = needles.iter().map(|needle| {
            let flags = if needle.len().saturating_mul(8).div_ceil(6) >= 8 {
                LITERAL | DECODED_LITERAL
            } else {
                LITERAL
            };
            (needle.as_slice(), flags)
        });
        let hex = hex_needles.iter().map(|needle| {
            let flags = if needle.len() >= encoded::STABLE_FLOOR {
                encoded::UNBOUNDED
            } else {
                encoded::BOUNDED_HEX
            };
            (needle.as_bytes(), flags)
        });
        let base64 = encoded_needles
            .iter()
            .map(|needle| (needle.value.as_bytes(), needle.flags));
        let raw = secrets
            .iter()
            .map(|secret| secret.expose_secret().as_bytes())
            .filter(|value| !value.is_empty())
            .map(|value| (value, LITERAL));
        let mut compiled_bytes = MAX_COMPILED_BYTES;
        let mut compile_work = 32 << 20;
        Some(Self {
            folded: Automaton::build(literal.chain(hex), &mut compiled_bytes, &mut compile_work)?,
            base64: Automaton::build(base64, &mut compiled_bytes, &mut compile_work)?,
            raw: Automaton::build(raw, &mut compiled_bytes, &mut compile_work)?,
            minimum_base64,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub(super) fn can_decode_base64(&self, len: usize, strict: bool) -> bool {
        self.minimum_base64[usize::from(strict)].is_some_and(|minimum| len >= minimum)
    }

    pub(super) fn raw(&self, input: &[u8], work: &mut usize) -> bool {
        !self.raw.is_empty() && self.raw.contains(input, work).unwrap_or(true)
    }

    pub(super) fn forms(
        &self,
        input: &[u8],
        decoded: bool,
        strict: bool,
        work: &mut usize,
    ) -> Option<bool> {
        self.scan(input, decoded, true, strict, work)
    }

    fn scan(
        &self,
        input: &[u8],
        decoded: bool,
        encodings: bool,
        strict: bool,
        work: &mut usize,
    ) -> Option<bool> {
        if self.is_empty() {
            return Some(false);
        }
        charge(work, input.len())?;
        let literals = if decoded && !strict {
            DECODED_LITERAL
        } else {
            LITERAL
        };
        let search_base64 = encodings && !self.base64.is_empty();
        let mut folded = 0;
        let mut base64 = 0;
        let starts = (input.len() >= SCAN_BLOCK).then(|| {
            let mut starts = Zeroizing::new(Vec::new());
            starts.extend((0..=u8::MAX).filter(|&byte| {
                self.folded.root(byte.to_ascii_lowercase()) != 0
                    || (search_base64 && self.base64.root(byte) != 0)
            }));
            starts
        });
        let mut repeated = None;
        for (block_index, block) in input.chunks(SCAN_BLOCK).enumerate() {
            let initial = (folded, base64);
            if let Some((previous, states, cost)) = repeated
                && initial == states
                && block == previous
            {
                charge(work, cost)?;
                continue;
            }
            if folded == 0
                && base64 == 0
                && block_excludes_starts(block, starts.as_deref().map(Vec::as_slice))
            {
                continue;
            }
            let before = *work;
            let mut saw_output = false;
            for (index, byte) in block.iter().copied().enumerate() {
                let end = block_index * SCAN_BLOCK + index + 1;
                let accepts_encoded = |length, flags| {
                    strict
                        || flags & encoded::UNBOUNDED != 0
                        || encoded::bounded(input, end - length, end, flags)
                };
                let lower = byte.to_ascii_lowercase();
                folded = if folded == 0 {
                    self.folded.root(lower)
                } else {
                    self.folded.advance(folded, lower, work)?
                };
                if folded != 0
                    && self.folded.accepts(folded, work, |length, flags| {
                        saw_output = true;
                        flags & literals != 0
                            || (encodings
                                && flags & (encoded::UNBOUNDED | encoded::BOUNDED_HEX) != 0
                                && accepts_encoded(length, flags))
                    })?
                {
                    return Some(true);
                }
                if search_base64 {
                    base64 = if base64 == 0 {
                        self.base64.root(byte)
                    } else {
                        self.base64.advance(base64, byte, work)?
                    };
                    if base64 != 0
                        && self.base64.accepts(base64, work, |length, flags| {
                            saw_output = true;
                            accepts_encoded(length, flags)
                        })?
                    {
                        return Some(true);
                    }
                }
            }
            repeated = (!saw_output && initial == (folded, base64) && block.len() == SCAN_BLOCK)
                .then_some((block, initial, before - *work));
        }
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_blocks_charge_the_same_failure_transitions() {
        let patterns = Patterns::compile(&[crate::Secret::new("toy-secret")]).unwrap();
        let mut block = br"\uD800".repeat(SCAN_BLOCK / 6);
        *block.last_mut().unwrap() = b'!';
        let body = block.repeat(3);
        let mut once = 10 * SCAN_BLOCK;
        assert_eq!(patterns.forms(&block, false, false, &mut once), Some(false));
        let mut thrice = 30 * SCAN_BLOCK;
        assert_eq!(
            patterns.forms(&body, false, false, &mut thrice),
            Some(false)
        );
        assert_eq!(thrice, once * 3);
    }

    #[test]
    fn block_prefilter_preserves_matches_and_work_limits_across_boundaries() {
        let credentials = [crate::Secret::new("toy-secret")];
        let patterns = Patterns::compile(&credentials).unwrap();
        let mut clean = vec![b'x'; 3 * SCAN_BLOCK];
        assert_eq!(
            patterns.forms(&clean, false, false, &mut (3 * SCAN_BLOCK - 1)),
            None
        );
        for credential in ["toy-secret", "TOY-SECRET", "dG95LXNlY3JldA=="] {
            let start = 2 * SCAN_BLOCK - 2;
            let end = start + credential.len();
            clean[start..end].copy_from_slice(credential.as_bytes());
            assert_eq!(
                patterns.forms(&clean, false, false, &mut (5 * SCAN_BLOCK)),
                Some(true)
            );
            assert_eq!(
                patterns.raw(&clean, &mut (5 * SCAN_BLOCK)),
                credential == "toy-secret"
            );
            clean[start..end].fill(b'x');
        }
    }

    #[test]
    fn compiler_refuses_when_automaton_storage_exceeds_four_mib() {
        let small = crate::Secret::new("x".repeat(8 << 10));
        let large = crate::Secret::new("x".repeat(64 << 10));
        assert!(Patterns::compile(&[small]).is_some());
        assert!(Patterns::compile(&[large]).is_none());
    }
}
