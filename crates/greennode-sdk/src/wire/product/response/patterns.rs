use super::encoded::{self, Base64Alphabet, EncodedNeedle};
use zeroize::Zeroizing;

pub(in crate::wire::product) struct Patterns {
    pub(super) literal: Vec<Zeroizing<Vec<u8>>>,
    pub(super) base64: Vec<EncodedNeedle>,
    pub(super) hex: Vec<Zeroizing<String>>,
}

struct PatternBudget {
    remaining_parts: usize,
    remaining_bytes: usize,
}

impl PatternBudget {
    fn charge(&mut self, len: usize) -> Option<()> {
        self.remaining_parts = self.remaining_parts.checked_sub(1)?;
        self.remaining_bytes = self.remaining_bytes.checked_sub(len)?;
        Some(())
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
                for (encoding, alphabet) in [
                    (data_encoding::BASE64, Base64Alphabet::Standard),
                    (data_encoding::BASE64_NOPAD, Base64Alphabet::Standard),
                    (data_encoding::BASE64URL, Base64Alphabet::UrlSafe),
                    (data_encoding::BASE64URL_NOPAD, Base64Alphabet::UrlSafe),
                ] {
                    EncodedNeedle::insert(
                        &mut encoded_needles,
                        Zeroizing::new(encoding.encode(value.as_bytes())),
                        alphabet,
                        false,
                    );
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
                                EncodedNeedle::insert(
                                    &mut encoded_needles,
                                    Zeroizing::new(stable.to_owned()),
                                    Base64Alphabet::Both,
                                    true,
                                );
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
        Some(Self {
            literal: needles,
            base64: encoded_needles,
            hex: hex_needles,
        })
    }
}
