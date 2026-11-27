use super::SentCredentials;
use crate::wire::product::response::BodyMatcher;
use url::Url;

pub(super) fn uri_contains_credential(url: &Url, credentials: &SentCredentials) -> bool {
    let path = url.path();
    let query = url.query();
    let length = path.len().saturating_add(query.map_or(0, str::len));
    let matcher = BodyMatcher::new(credentials, length);
    matcher.raw_or_percent_decoded(path.as_bytes())
        || query.is_some_and(|part| matcher.raw_or_percent_decoded(part.as_bytes()))
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
