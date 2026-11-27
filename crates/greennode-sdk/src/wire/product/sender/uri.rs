use super::SentCredentials;
use url::Url;

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn contains_in_component(component: &str, credentials: &SentCredentials) -> bool {
    let raw = component.as_bytes();
    if credentials.reflects(raw) {
        return true;
    }
    let mut decoded = zeroize::Zeroizing::new(Vec::with_capacity(raw.len()));
    let mut cursor = 0;
    while let Some(&byte) = raw.get(cursor) {
        if byte == b'%'
            && let (Some(a), Some(b)) = (
                raw.get(cursor + 1).and_then(|b| hex(*b)),
                raw.get(cursor + 2).and_then(|b| hex(*b)),
            )
        {
            decoded.push((a << 4) | b);
            cursor += 3;
        } else {
            decoded.push(byte);
            cursor += 1;
        }
    }
    credentials.reflects(&decoded)
}
pub(super) fn uri_contains_credential(url: &Url, credentials: &SentCredentials) -> bool {
    contains_in_component(url.path(), credentials)
        || url
            .query()
            .is_some_and(|query| contains_in_component(query, credentials))
}
pub(in crate::wire::product) fn uri_reflects_credential_forms(
    url: &Url,
    credentials: &SentCredentials,
) -> bool {
    let reflects = |component| {
        crate::wire::product::response::reflects_credential_forms(component, credentials)
    };
    !credentials.values().is_empty() && (reflects(url.path()) || url.query().is_some_and(reflects))
}
