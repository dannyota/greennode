use crate::Secret;
use crate::wire::product::evidence::Evidence;
use crate::wire::product::response::percent_decode;

pub(super) fn first_json_value(input: &str) -> Option<&serde_json::value::RawValue> {
    serde::Deserialize::deserialize(&mut serde_json::Deserializer::from_str(input)).ok()
}

pub(super) fn collect_url(
    evidence: &mut Evidence,
    value: &str,
    include_query: impl Fn(&str) -> bool,
) -> Result<(), ()> {
    let url = url::Url::parse(value).or_else(|error| {
        if error != url::ParseError::RelativeUrlWithoutBase {
            return Err(error);
        }
        let base = url::Url::parse("https://example.invalid/")?;
        base.join(value)
    });
    let Ok(url) = url else {
        return Ok(());
    };
    for value in [Some(url.username()), url.password()].into_iter().flatten() {
        let decoded = percent_decode(value.as_bytes());
        evidence.add_redaction(&Secret::new(String::from_utf8_lossy(&decoded).into_owned()));
    }
    for (key, value) in url.query_pairs() {
        if include_query(&key) {
            evidence.add_redaction(&Secret::new(value.into_owned()));
            check_budget(evidence)?;
        }
    }
    check_budget(evidence)
}

pub(super) fn check_budget(evidence: &Evidence) -> Result<(), ()> {
    if evidence.credentials().values().len() > 256 {
        return Err(());
    }
    Ok(())
}
