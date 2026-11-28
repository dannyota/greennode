use super::{
    automaton::{Automaton, SCAN_BLOCK, block_excludes_starts, charge},
    encoded::{self, EncodedNeedle},
};
use zeroize::Zeroizing;

#[cfg(feature = "test-support")]
static ALLOCATION_TRACE: std::sync::OnceLock<fn(u8)> = std::sync::OnceLock::new();

#[cfg(feature = "test-support")]
pub(crate) fn set_allocation_trace(trace: fn(u8)) {
    let _ = ALLOCATION_TRACE.set(trace);
}

#[cfg(feature = "test-support")]
fn trace(phase: u8) {
    if let Some(trace) = ALLOCATION_TRACE.get() {
        trace(phase);
    }
}

const LITERAL: u8 = 1;
const DECODED_LITERAL: u8 = 1 << 1;
const MAX_COMPILED_BYTES: usize = 64 << 20;
const GROUP_SIZE: usize = 8;
const MAX_PARTS: usize = (256 << 10) / 8 + 1024;

struct Compiled {
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

fn credential_parts(value: &str) -> impl Iterator<Item = &str> + Clone {
    std::iter::once(value)
        .chain(
            value
                .lines()
                .map(str::trim)
                .filter(move |line| line.len() >= 8 && *line != value),
        )
        .filter(|part| !part.is_empty())
}

fn redundant_bearer_header(value: &str, secrets: &[crate::Secret]) -> bool {
    value.strip_prefix("Bearer ").is_some_and(|token| {
        token.len() >= 6 && secrets.iter().any(|secret| secret.expose_secret() == token)
    })
}

#[derive(Clone)]
pub(in crate::wire::product) struct Patterns {
    groups: Vec<std::sync::Arc<PatternGroup>>,
    parts: usize,
    bytes: usize,
    peak: usize,
}

struct PatternGroup {
    secrets: Vec<crate::Secret>,
    compiled: Compiled,
    bytes: usize,
}

impl Patterns {
    pub(in crate::wire::product) fn compile(secrets: &[crate::Secret]) -> Option<Self> {
        let mut result = Self {
            groups: Vec::new(),
            parts: 0,
            bytes: 0,
            peak: 0,
        };
        for (index, secret) in secrets.iter().enumerate() {
            result = result.append(secret, &secrets[..=index])?;
        }
        Some(result)
    }

    pub(in crate::wire::product) fn append(
        &self,
        secret: &crate::Secret,
        known: &[crate::Secret],
    ) -> Option<Self> {
        self.append_with_size(secret, known, GROUP_SIZE)
    }

    fn append_with_size(
        &self,
        secret: &crate::Secret,
        known: &[crate::Secret],
        group_size: usize,
    ) -> Option<Self> {
        let value = secret.expose_secret();
        let redundant = redundant_bearer_header(value, known);
        let parts = 1
            + usize::from(!value.is_empty() && !redundant)
            + if redundant {
                0
            } else {
                value
                    .lines()
                    .map(str::trim)
                    .filter(|line| line.len() >= 8 && *line != value)
                    .count()
            };
        let part_bytes = if redundant {
            value.len()
        } else {
            value.len().checked_add(
                value
                    .lines()
                    .map(str::trim)
                    .filter(|line| line.len() >= 8 && *line != value)
                    .try_fold(value.len(), |total, line| total.checked_add(line.len()))?,
            )?
        };
        let total_parts = self.parts.checked_add(parts)?;
        let total_bytes = self.bytes.checked_add(part_bytes)?;
        if total_parts > MAX_PARTS || total_bytes > 4 << 20 {
            return None;
        }
        #[cfg(feature = "test-support")]
        trace(1);
        let mut groups = self.groups.clone();
        let mut peak = self.peak;
        if !redundant && !value.is_empty() {
            let mut combined = vec![secret.clone()];
            let mut overlap = 0usize;
            if groups.last().is_some_and(|group| {
                group.secrets.len() < group_size
                    && group
                        .secrets
                        .iter()
                        .map(|secret| secret.expose_secret().len())
                        .sum::<usize>()
                        .saturating_add(value.len())
                        <= group_size.saturating_mul(1024)
            }) {
                let group = groups.pop()?;
                overlap = group.bytes;
                combined.extend(group.secrets.iter().cloned());
            }
            let storage = total_bytes.checked_add(total_parts.checked_mul(64)?)?;
            let used = groups
                .iter()
                .try_fold(overlap.checked_add(storage)?, |sum, group| {
                    sum.checked_add(group.bytes)
                })?;
            let available = MAX_COMPILED_BYTES.checked_sub(used)?;
            let (compiled, bytes, build_peak) = Compiled::compile_accounted(&combined, available)?;
            peak = peak.max(used.checked_add(build_peak)?);
            groups.push(std::sync::Arc::new(PatternGroup {
                secrets: combined,
                compiled,
                bytes,
            }));
        }
        let storage = total_bytes.checked_add(total_parts.checked_mul(64)?)?;
        let current = groups
            .iter()
            .try_fold(storage, |sum, group| sum.checked_add(group.bytes))?;
        if current > MAX_COMPILED_BYTES {
            return None;
        }
        peak = peak.max(current);
        Some(Self {
            groups,
            parts: total_parts,
            bytes: total_bytes,
            peak,
        })
    }

    #[cfg(test)]
    fn compile_accounted(secrets: &[crate::Secret]) -> Option<(Self, usize)> {
        let result = Self::compile(secrets)?;
        let used = result.peak;
        Some((result, used))
    }

    #[cfg(feature = "test-support")]
    pub(in crate::wire::product) fn peak_bytes(&self) -> usize {
        self.peak
    }
    pub(super) fn group_count(&self) -> usize {
        self.groups.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.compiled.is_empty())
    }
    pub(super) fn can_decode_base64(&self, len: usize, strict: bool) -> bool {
        self.groups
            .iter()
            .any(|group| group.compiled.can_decode_base64(len, strict))
    }
    pub(super) fn raw(&self, input: &[u8], work: &mut usize) -> bool {
        if self.excludes_starts(input, true) {
            return charge(work, input.len()).is_none();
        }
        self.groups
            .iter()
            .any(|group| group.compiled.raw(input, work))
    }
    fn excludes_starts(&self, input: &[u8], raw: bool) -> bool {
        if input.len() < SCAN_BLOCK {
            return false;
        }
        let mut starts = [false; 256];
        for (byte, present) in starts.iter_mut().enumerate() {
            let byte = byte as u8;
            *present = self.groups.iter().any(|group| {
                if raw {
                    group.compiled.raw.root(byte) != 0
                } else {
                    group.compiled.folded.root(byte.to_ascii_lowercase()) != 0
                        || group.compiled.base64.root(byte) != 0
                }
            });
        }
        input.iter().all(|byte| !starts[usize::from(*byte)])
    }
    pub(super) fn forms(
        &self,
        input: &[u8],
        decoded: bool,
        strict: bool,
        work: &mut usize,
    ) -> Option<bool> {
        if self.excludes_starts(input, false) {
            charge(work, input.len())?;
            return Some(false);
        }
        let mut indeterminate = false;
        for group in &self.groups {
            match group.compiled.forms(input, decoded, strict, work) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => indeterminate = true,
            }
        }
        if indeterminate { None } else { Some(false) }
    }
}

impl Compiled {
    fn compile_accounted(secrets: &[crate::Secret], limit: usize) -> Option<(Self, usize, usize)> {
        let mut budget = PatternBudget {
            remaining_parts: MAX_PARTS,
            remaining_bytes: 4 << 20,
        };
        let eligible = || {
            secrets
                .iter()
                .map(|secret| secret.expose_secret())
                .filter(|value| !redundant_bearer_header(value, secrets))
        };
        let parts = || eligible().flat_map(credential_parts);
        let mut maximum = 0usize;
        for value in secrets {
            let value = value.expose_secret();
            budget.charge(value.len())?;
            if redundant_bearer_header(value, secrets) {
                continue;
            }
            for part in credential_parts(value) {
                budget.charge(part.len())?;
                maximum = maximum.max(part.len());
            }
        }
        let minimum_base64 = [
            parts()
                .map(|part| part.len().saturating_mul(8).div_ceil(6))
                .filter(|length| *length >= 7)
                .min(),
            parts()
                .map(|part| part.len().saturating_mul(8).div_ceil(6))
                .min(),
        ];
        let temporary = maximum.checked_mul(4)?.checked_add(128)?;
        let mut compiled_bytes = limit;
        charge(&mut compiled_bytes, temporary)?;
        let mut lowest = compiled_bytes;
        let mut compile_work = 32 << 20;
        #[cfg(feature = "test-support")]
        trace(2);
        let literal = parts().map(|part| {
            let value = Zeroizing::new(part.as_bytes().to_ascii_lowercase());
            let flags = if value.len().saturating_mul(8).div_ceil(6) >= 8 {
                LITERAL | DECODED_LITERAL
            } else {
                LITERAL
            };
            (value, flags)
        });
        let hex = parts().map(|part| {
            let value =
                Zeroizing::new(data_encoding::HEXLOWER.encode(part.as_bytes()).into_bytes());
            let flags = if value.len() >= encoded::STABLE_FLOOR {
                encoded::UNBOUNDED
            } else {
                encoded::BOUNDED_HEX
            };
            (value, flags)
        });
        #[cfg(feature = "test-support")]
        trace(3);
        let folded = Automaton::build_with_peak(
            literal.chain(hex),
            &mut compiled_bytes,
            &mut compile_work,
            &mut lowest,
        )?;
        let short = eligible()
            .filter(|value| !value.is_empty() && value.len() < 6)
            .flat_map(|value| {
                [
                    (data_encoding::BASE64, false),
                    (data_encoding::BASE64_NOPAD, false),
                    (data_encoding::BASE64URL, true),
                    (data_encoding::BASE64URL_NOPAD, true),
                ]
                .into_iter()
                .map(move |(encoding, url_safe)| {
                    let needle = EncodedNeedle::short(
                        Zeroizing::new(encoding.encode(value.as_bytes())),
                        url_safe,
                    );
                    (
                        Zeroizing::new(needle.value.as_bytes().to_vec()),
                        needle.flags,
                    )
                })
            });
        let fragments = parts().flat_map(|part| {
            (0..3).flat_map(move |offset| {
                [data_encoding::BASE64, data_encoding::BASE64URL]
                    .into_iter()
                    .filter_map(move |encoding| {
                        let mut aligned = Zeroizing::new(vec![0; offset]);
                        aligned.extend_from_slice(part.as_bytes());
                        let start = (offset * 8).div_ceil(6);
                        let end = offset.saturating_add(part.len()).saturating_mul(8) / 6;
                        if end.saturating_sub(start) < encoded::STABLE_FLOOR {
                            return None;
                        }
                        let encoded = Zeroizing::new(encoding.encode(&aligned));
                        Some((
                            Zeroizing::new(encoded.get(start..end)?.as_bytes().to_vec()),
                            encoded::UNBOUNDED,
                        ))
                    })
            })
        });
        #[cfg(feature = "test-support")]
        trace(4);
        let base64 = Automaton::build_with_peak(
            short.chain(fragments),
            &mut compiled_bytes,
            &mut compile_work,
            &mut lowest,
        )?;
        let raw = eligible()
            .filter(|value| !value.is_empty())
            .map(|value| (value.as_bytes(), LITERAL));
        #[cfg(feature = "test-support")]
        trace(5);
        let raw =
            Automaton::build_with_peak(raw, &mut compiled_bytes, &mut compile_work, &mut lowest)?;
        let used = limit.checked_sub(compiled_bytes)?.checked_sub(temporary)?;
        let peak = limit.checked_sub(lowest)?;
        Some((
            Self {
                folded,
                base64,
                raw,
                minimum_base64,
            },
            used,
            peak,
        ))
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
    fn compiler_accepts_256_bearers_and_wire_headers() {
        let secrets: Vec<_> = (1..=256)
            .flat_map(|index| {
                [
                    crate::Secret::new(format!("toy-rotating-bearer-{index}")),
                    crate::Secret::new(format!("Bearer toy-rotating-bearer-{index}")),
                ]
            })
            .collect();
        assert!(Patterns::compile(&secrets).is_some());
    }

    #[test]
    fn compiler_accepts_256_long_bearers_and_wire_headers() {
        let secrets: Vec<_> = (1..=256)
            .flat_map(|index| {
                let value = format!("{}-{index}", "toy-credential".repeat(8));
                [
                    crate::Secret::new(&value),
                    crate::Secret::new(format!("Bearer {value}")),
                ]
            })
            .collect();
        assert!(Patterns::compile(&secrets).is_some());
    }

    #[test]
    fn grouped_patterns_match_the_single_automaton() {
        let secrets: Vec<_> = (0..19)
            .map(|index| crate::Secret::new(format!("toy-secret-{index:02}")))
            .collect();
        let grouped = Patterns::compile(&secrets).unwrap();
        let (single, _, _) = Compiled::compile_accounted(&secrets, 64 << 20).unwrap();
        let mut inputs = vec![
            "ordinary text".to_owned(),
            "contains TOY-SECRET-03".to_owned(),
        ];
        for secret in &secrets {
            let bytes = secret.expose_secret().as_bytes();
            inputs.push(data_encoding::HEXLOWER.encode(bytes));
            inputs.push(data_encoding::BASE64.encode(bytes));
            inputs.push(data_encoding::BASE64URL_NOPAD.encode(bytes));
        }
        for input in inputs {
            for strict in [false, true] {
                let mut one_work = 8 << 20;
                let mut group_work = 8 << 20;
                assert_eq!(
                    grouped.forms(input.as_bytes(), false, strict, &mut group_work),
                    single.forms(input.as_bytes(), false, strict, &mut one_work)
                );
            }
        }
    }

    #[test]
    fn shared_bearer_forms_preserve_literal_hex_and_base64_matches() {
        for token in ["abcde", "abcdef", "toy-credential"] {
            let wire = format!("Bearer {token}");
            let standalone =
                super::super::SentCredentials::for_test(vec![crate::Secret::new(&wire)]);
            let paired = super::super::SentCredentials::for_test(vec![
                crate::Secret::new(token),
                crate::Secret::new(&wire),
            ]);
            for encoded in [
                wire.clone(),
                data_encoding::HEXLOWER.encode(wire.as_bytes()),
                data_encoding::BASE64.encode(wire.as_bytes()),
                data_encoding::BASE64URL_NOPAD.encode(wire.as_bytes()),
            ] {
                assert!(super::super::reflects_credential(&encoded, &standalone));
                assert!(super::super::reflects_credential(&encoded, &paired));
            }
        }
    }

    #[test]
    fn compiler_accepts_256_kib_of_distinct_bearers() {
        let mut state = 0x5eed_u64;
        let secrets: Vec<_> = (0..256)
            .flat_map(|_| {
                let token: String = (0..1024)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        char::from(33 + (state % 94) as u8)
                    })
                    .collect();
                [
                    crate::Secret::new(&token),
                    crate::Secret::new(format!("Bearer {token}")),
                ]
            })
            .collect();
        let (_, bytes) = Patterns::compile_accounted(&secrets).unwrap();
        assert!(bytes <= 64 << 20);
        let mut body = vec![b'a'; 8 << 20];
        body[0] = b'"';
        let last = body.len() - 1;
        body[last] = b'"';
        for group_size in [8, 16] {
            let mut patterns = Patterns {
                groups: Vec::new(),
                parts: 0,
                bytes: 0,
                peak: 0,
            };
            for (index, secret) in secrets.iter().enumerate() {
                patterns = patterns
                    .append_with_size(secret, &secrets[..=index], group_size)
                    .unwrap();
            }
            assert_eq!(patterns.group_count(), 256 / group_size);
            let credentials =
                super::super::SentCredentials::for_test_with_patterns(secrets.clone(), patterns);
            let evidence = crate::wire::product::evidence::Evidence::from_credentials(&credentials);
            assert!(evidence.peer_body_screen(&body).is_clean());
        }
    }

    #[test]
    fn compiler_accepts_repeated_large_bearers_within_byte_limit() {
        let secrets: Vec<_> = (0..4)
            .flat_map(|index| {
                let token = format!("wide-{index}-{}", "x".repeat(60 << 10));
                [
                    crate::Secret::new(&token),
                    crate::Secret::new(format!("Bearer {token}")),
                ]
            })
            .collect();
        assert!(Patterns::compile(&secrets).is_some());
    }

    #[test]
    fn compiler_accepts_four_diverse_64_kib_bearers() {
        let mut state = 0x12345678_u64;
        let secrets: Vec<_> = (0..4)
            .flat_map(|_| {
                let token: String = (0..(64 << 10))
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        char::from(33 + (state % 94) as u8)
                    })
                    .collect();
                [
                    crate::Secret::new(&token),
                    crate::Secret::new(format!("Bearer {token}")),
                ]
            })
            .collect();
        let (_, bytes) = Patterns::compile_accounted(&secrets).unwrap();
        assert!(bytes <= MAX_COMPILED_BYTES);
    }

    #[test]
    fn compiler_accepts_one_maximum_size_bearer() {
        let mut state = 0x5eed_u64;
        let token: String = (0..(256 << 10))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(33 + (state % 94) as u8)
            })
            .collect();
        let secrets = [
            crate::Secret::new(&token),
            crate::Secret::new(format!("Bearer {token}")),
        ];
        assert!(Patterns::compile(&secrets).is_some());
    }

    #[test]
    fn compiler_refuses_input_exceeding_its_own_budget() {
        let small = crate::Secret::new("x".repeat(8 << 10));
        let large = crate::Secret::new("x".repeat(4 << 20));
        assert!(Patterns::compile(&[small]).is_some());
        assert!(Patterns::compile(&[large]).is_none());
    }
}
