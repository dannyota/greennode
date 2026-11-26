use super::fold_table::RANGES;

pub(super) fn fold(value: &str) -> impl Iterator<Item = char> + '_ {
    value.chars().map(fold_char)
}

pub(super) fn same_alias(left: &str, right: &str) -> bool {
    fold(left).eq(fold(right))
}

fn fold_char(value: char) -> char {
    if value.is_ascii() {
        return value.to_ascii_uppercase();
    }
    let code = u32::from(value);
    let range = RANGES.partition_point(|range| range.1 < code);
    let Some(&(start, _, stride, offset)) = RANGES.get(range) else {
        return value;
    };
    if code >= start && (code - start).is_multiple_of(stride) {
        char::from_u32(code.wrapping_add_signed(offset)).unwrap_or(value)
    } else {
        value
    }
}
