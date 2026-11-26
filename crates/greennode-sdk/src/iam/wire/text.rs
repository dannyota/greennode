use std::borrow::Cow;

pub(super) fn go_json(bytes: &[u8]) -> Cow<'_, str> {
    let text = go_utf8(bytes);
    let bytes = text.as_bytes();
    let mut normalized = String::new();
    let mut start = 0;
    let mut index = 0;
    let mut in_string = false;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'"' {
            in_string = !in_string;
        }
        if !in_string || byte != b'\\' {
            index += 1;
            continue;
        }
        let Some(unit) = utf16_escape(bytes, index) else {
            index += 2;
            continue;
        };
        let end = index + 6;
        if (0xD800..=0xDBFF).contains(&unit)
            && utf16_escape(bytes, end).is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
        {
            index = end + 6;
            continue;
        }
        if (0xD800..=0xDFFF).contains(&unit) {
            if let Some(prefix) = text.get(start..index) {
                normalized.push_str(prefix);
            }
            normalized.push(char::REPLACEMENT_CHARACTER);
            start = end;
        }
        index = end;
    }
    if start == 0 {
        return text;
    }
    if let Some(rest) = text.get(start..) {
        normalized.push_str(rest);
    }
    Cow::Owned(normalized)
}

fn utf16_escape(bytes: &[u8], offset: usize) -> Option<u16> {
    let digits = bytes.get(offset..offset + 6)?.strip_prefix(b"\\u")?;
    let digits = std::str::from_utf8(digits).ok()?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

fn go_utf8(bytes: &[u8]) -> Cow<'_, str> {
    let mut chunks = bytes.utf8_chunks();
    let Some(first) = chunks.next() else {
        return Cow::Borrowed("");
    };
    if first.invalid().is_empty() {
        return Cow::Borrowed(first.valid());
    }
    let mut normalized = String::with_capacity(bytes.len());
    for chunk in std::iter::once(first).chain(chunks) {
        normalized.push_str(chunk.valid());
        normalized.extend(std::iter::repeat_n(
            char::REPLACEMENT_CHARACTER,
            chunk.invalid().len(),
        ));
    }
    Cow::Owned(normalized)
}
