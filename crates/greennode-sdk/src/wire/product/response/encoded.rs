use zeroize::Zeroizing;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Base64Alphabet {
    Standard,
    UrlSafe,
    Both,
}

pub(super) struct EncodedNeedle {
    value: Zeroizing<String>,
    alphabet: Base64Alphabet,
}

impl EncodedNeedle {
    pub(super) fn len(&self) -> usize {
        self.value.len()
    }

    pub(super) fn insert(
        needles: &mut Vec<Self>,
        value: Zeroizing<String>,
        alphabet: Base64Alphabet,
    ) {
        if let Some(needle) = needles
            .iter_mut()
            .find(|needle| needle.value.as_str() == value.as_str())
        {
            if needle.alphabet != alphabet {
                needle.alphabet = Base64Alphabet::Both;
            }
        } else {
            needles.push(Self { value, alphabet });
        }
    }

    pub(super) fn matches(&self, input: &[u8], strict: bool) -> bool {
        let needle = self.value.as_str();
        input.utf8_chunks().any(|chunk| {
            let text = chunk.valid();
            text.match_indices(needle).any(|(start, _)| {
                if strict || needle.len() >= 8 {
                    return true;
                }
                let left = start
                    .checked_sub(1)
                    .and_then(|index| text.as_bytes().get(index));
                let right = text.as_bytes().get(start + needle.len());
                let bounded = |symbols: &[u8]| {
                    [left, right]
                        .into_iter()
                        .flatten()
                        .all(|byte| !byte.is_ascii_alphanumeric() && !symbols.contains(byte))
                };
                (matches!(
                    self.alphabet,
                    Base64Alphabet::Standard | Base64Alphabet::Both
                ) && bounded(b"+/"))
                    || (matches!(
                        self.alphabet,
                        Base64Alphabet::UrlSafe | Base64Alphabet::Both
                    ) && bounded(b"-_"))
            })
        })
    }
}

pub(super) fn contains_hex(input: &[u8], needle: &str) -> bool {
    if needle.len() < 8 {
        input == needle.as_bytes()
    } else {
        input
            .utf8_chunks()
            .any(|chunk| chunk.valid().contains(needle))
    }
}

pub(super) fn insert_hex(needles: &mut Vec<Zeroizing<String>>, part: &str) {
    let hex = Zeroizing::new(data_encoding::HEXLOWER.encode(part.as_bytes()));
    if !needles.contains(&hex) {
        needles.push(hex);
    }
}
