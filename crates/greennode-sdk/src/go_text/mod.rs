mod html;
mod quote;
mod tables;
#[cfg(test)]
mod tests;

pub(crate) use html::unescape_html;
pub(crate) use quote::go_quote;

pub(crate) fn credential_key_byte(ch: char) -> Option<u8> {
    if ch.is_ascii() {
        return ch
            .is_ascii_alphanumeric()
            .then(|| ch.to_ascii_lowercase() as u8);
    }
    tables::is_letter_or_digit(ch).then(|| tables::ascii_lower(ch).unwrap_or(0))
}
