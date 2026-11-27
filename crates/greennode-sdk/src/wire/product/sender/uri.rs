use super::SentCredentials;
use crate::Secret;
use url::Url;

pub(super) fn contains_secret(data: &[u8], secret: &Secret) -> bool {
    let value = secret.expose_secret().as_bytes();
    !value.is_empty() && data.windows(value.len()).any(|part| part == value)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn contains_in_component(component: &str, prior: &SentCredentials, pending: &[&Secret]) -> bool {
    let raw = component.as_bytes();
    if prior.reflects(raw) || pending.iter().any(|secret| contains_secret(raw, secret)) {
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
    prior.reflects(&decoded)
        || pending
            .iter()
            .any(|secret| contains_secret(&decoded, secret))
}
pub(super) fn uri_contains_credential(
    url: &Url,
    prior: &SentCredentials,
    pending: &[&Secret],
) -> bool {
    contains_in_component(url.path(), prior, pending)
        || url
            .query()
            .is_some_and(|query| contains_in_component(query, prior, pending))
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
