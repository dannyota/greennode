use super::tables::{entities, replacement};

const LONGEST_WITHOUT_SEMICOLON: usize = 6;

pub(crate) fn unescape_html(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut src = 0;
    while src < bytes.len() {
        let Some(offset) = bytes[src..].iter().position(|byte| *byte == b'&') else {
            out.push_str(&text[src..]);
            break;
        };
        out.push_str(&text[src..src + offset]);
        src += offset;
        src += entity(&text[src..], &mut out);
    }
    out
}

fn entity(rest: &str, out: &mut String) -> usize {
    let bytes = rest.as_bytes();
    let literal = |out: &mut String| {
        out.push('&');
        1
    };
    if bytes.len() <= 1 {
        return literal(out);
    }
    let mut i = 1;
    if bytes[i] == b'#' {
        if bytes.len() <= 3 {
            return literal(out);
        }
        i += 1;
        let hex = matches!(bytes[i], b'x' | b'X');
        if hex {
            i += 1;
        }
        let mut value: i32 = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            i += 1;
            let digit = match byte {
                b'0'..=b'9' => Some(i32::from(byte - b'0')),
                b'a'..=b'f' if hex => Some(i32::from(byte - b'a') + 10),
                b'A'..=b'F' if hex => Some(i32::from(byte - b'A') + 10),
                _ => None,
            };
            if let Some(digit) = digit {
                value = value
                    .wrapping_mul(if hex { 16 } else { 10 })
                    .wrapping_add(digit);
                continue;
            }
            if byte != b';' {
                i -= 1;
            }
            break;
        }
        if i <= 3 {
            return literal(out);
        }
        out.push(numeric(value));
        return i;
    }
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if byte.is_ascii_alphanumeric() {
            continue;
        }
        if byte != b';' {
            i -= 1;
        }
        break;
    }
    let name = &rest[1..i];
    let table = entities();
    if name.is_empty() {
    } else if let Some(&(first, second)) = table.get(name) {
        out.push(first);
        out.extend(second);
        return i;
    } else {
        let max = (name.len() - 1).min(LONGEST_WITHOUT_SEMICOLON);
        for j in (2..=max).rev() {
            if let Some(&(first, None)) = table.get(&name[..j]) {
                out.push(first);
                return j + 1;
            }
        }
    }
    out.push_str(&rest[..i]);
    i
}

fn numeric(value: i32) -> char {
    match value {
        0x80..=0x9F => replacement(usize::try_from(value - 0x80).unwrap_or(0)),
        _ => u32::try_from(value)
            .ok()
            .and_then(char::from_u32)
            .filter(|ch| *ch != '\0')
            .unwrap_or(char::REPLACEMENT_CHARACTER),
    }
}
