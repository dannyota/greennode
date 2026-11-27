use zeroize::Zeroizing;

pub(super) const STABLE_FLOOR: usize = 7;
const WHOLE_FLOOR: usize = 8;
pub(super) const UNBOUNDED: u8 = 1 << 2;
pub(super) const BOUNDED_HEX: u8 = 1 << 3;
const STANDARD: u8 = 1 << 4;
const URL_SAFE: u8 = 1 << 5;

pub(super) struct EncodedNeedle {
    pub(super) value: Zeroizing<String>,
    pub(super) flags: u8,
}

impl EncodedNeedle {
    pub(super) fn short(value: Zeroizing<String>, url_safe: bool) -> Self {
        let flags = if value.len() >= WHOLE_FLOOR {
            UNBOUNDED
        } else if url_safe {
            URL_SAFE
        } else {
            STANDARD
        };
        Self { value, flags }
    }

    pub(super) fn fragment(value: Zeroizing<String>) -> Self {
        Self {
            value,
            flags: UNBOUNDED,
        }
    }

    pub(super) fn dedup(needles: &mut Vec<Self>) {
        needles.sort_unstable_by(|a, b| a.value.as_str().cmp(b.value.as_str()));
        needles.dedup_by(|next, kept| {
            if next.value != kept.value {
                return false;
            }
            kept.flags |= next.flags;
            true
        });
    }
}

pub(super) fn bounded(input: &[u8], start: usize, end: usize, flags: u8) -> bool {
    let left = start.checked_sub(1).map(|index| input[index]);
    let right = input.get(end).copied();
    let outside = |symbols: &[u8]| {
        [left, right]
            .into_iter()
            .flatten()
            .all(|byte| !byte.is_ascii_alphanumeric() && !symbols.contains(&byte))
    };
    (flags & BOUNDED_HEX != 0 && outside(b""))
        || (flags & STANDARD != 0 && outside(b"+/"))
        || (flags & URL_SAFE != 0 && outside(b"-_"))
}
