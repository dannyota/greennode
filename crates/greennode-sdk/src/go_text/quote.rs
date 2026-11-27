use super::tables::is_print;
use std::fmt::Write;

pub(crate) fn go_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ if is_print(ch) => out.push(ch),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            _ if ch < ' ' || ch == '\u{7f}' => {
                let _ = write!(out, "\\x{:02x}", u32::from(ch));
            }
            _ if u32::from(ch) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            _ => {
                let _ = write!(out, "\\U{:08x}", u32::from(ch));
            }
        }
    }
    out.push('"');
    out
}
