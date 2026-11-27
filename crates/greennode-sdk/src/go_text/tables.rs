use std::collections::HashMap;
use std::sync::OnceLock;

type Entities = HashMap<&'static str, (char, Option<char>)>;

fn rows(text: &'static str) -> impl Iterator<Item = &'static str> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

fn scalar(hex: &str) -> u32 {
    u32::from_str_radix(hex, 16).unwrap_or(0xFFFD)
}

fn code(hex: &str) -> char {
    char::from_u32(scalar(hex)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

pub(super) fn entities() -> &'static Entities {
    static TABLE: OnceLock<Entities> = OnceLock::new();
    TABLE.get_or_init(|| {
        rows(include_str!("entities.txt"))
            .filter_map(|row| {
                let mut fields = row.split(' ');
                let name = fields.next()?;
                let first = code(fields.next()?);
                Some((name, (first, fields.next().map(code))))
            })
            .collect()
    })
}

pub(super) fn replacement(index: usize) -> char {
    static TABLE: OnceLock<Vec<char>> = OnceLock::new();
    TABLE
        .get_or_init(|| rows(include_str!("replacements.txt")).map(code).collect())
        .get(index)
        .copied()
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

pub(super) fn is_print(ch: char) -> bool {
    static TABLE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        rows(include_str!("printable.txt"))
            .filter_map(|row| {
                let (first, last) = row.split_once(' ')?;
                Some((scalar(first), scalar(last)))
            })
            .collect()
    });
    let value = u32::from(ch);
    table
        .binary_search_by(|&(first, last)| {
            if value < first {
                std::cmp::Ordering::Greater
            } else if value > last {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
