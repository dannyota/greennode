mod html;
mod quote;
mod tables;
#[cfg(test)]
mod tests;

pub(crate) use html::unescape_html;
pub(crate) use quote::go_quote;

pub(crate) fn ascii_lowercase_for_key(ch: char) -> char {
    if ch.is_ascii() {
        ch.to_ascii_lowercase()
    } else {
        tables::ascii_lower(ch).map_or(ch, char::from)
    }
}
