use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::validate;

pub(crate) fn string(bytes: &[u8], index: &mut usize) -> Option<String> {
    let start = *index;
    if bytes.get(*index) != Some(&b'"') {
        return None;
    }
    *index += 1;
    loop {
        match bytes.get(*index).copied()? {
            b'\\' => *index = index.checked_add(2)?,
            b'"' => {
                *index += 1;
                break;
            }
            _ => *index += 1,
        }
    }
    let normalized = crate::model::wire::go_json(bytes.get(start..*index)?);
    serde_json::from_str(&normalized).ok()
}

pub(crate) fn number(bytes: &[u8], index: &mut usize) -> Option<usize> {
    let start = *index;
    while bytes
        .get(*index)
        .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'0'..=b'9' | b'.' | b'e' | b'E'))
    {
        *index += 1;
    }
    (*index > start).then_some(*index - start)
}

pub(crate) fn redact(raw: &str, evidence: &Evidence) -> Option<String> {
    validate(raw.as_bytes(), &[], raw.len()).ok()?;
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut previous = 0;
    let mut result = String::new();
    while let Some(byte) = bytes.get(index).copied() {
        let start = index;
        let reflected = match byte {
            b'"' => evidence.text_reflects(&string(bytes, &mut index)?),
            b'-' | b'0'..=b'9' => {
                number(bytes, &mut index)?;
                evidence.text_reflects(raw.get(start..index)?)
            }
            b't' | b'f' | b'n' => {
                while bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
                    index += 1;
                }
                evidence.text_reflects(raw.get(start..index)?)
            }
            _ => {
                index += 1;
                false
            }
        };
        if reflected {
            if evidence.text_reflects("<redacted>") {
                return None;
            }
            result.push_str(raw.get(previous..start)?);
            result.push_str("\"<redacted>\"");
            previous = index;
        }
    }
    result.push_str(raw.get(previous..)?);
    Some(result)
}
