use super::{ChannelHeader, Evidence, Secret};
use serde_json::value::RawValue;

pub(super) fn channel_values(
    address: &str,
    headers: &[ChannelHeader],
    raw_header: &str,
    reference: &str,
    otp: &Secret,
) -> Evidence {
    let mut evidence = Evidence::new();
    evidence.add_redaction(otp);
    for value in [raw_header, reference, address]
        .into_iter()
        .chain(headers.iter().map(|header| header.value.as_str()))
    {
        add(&mut evidence, value);
    }
    if headers.is_empty()
        && let Ok(raw) = serde_json::from_str::<&RawValue>(raw_header)
    {
        visit(&mut evidence, raw, 128);
    }
    evidence
}

fn add(evidence: &mut Evidence, value: &str) {
    if evidence.credentials().values().len() > 256 {
        return;
    }
    evidence.add_redaction(&Secret::new(value));
    if let Ok(url) = url::Url::parse(value) {
        evidence.add_redaction(&Secret::new(url.username()));
        if let Some(password) = url.password() {
            evidence.add_redaction(&Secret::new(password));
        }
        for (_, value) in url.query_pairs().take(257) {
            evidence.add_redaction(&Secret::new(value.into_owned()));
        }
    }
    if let Some((scheme, value)) = value.trim().split_once(' ') {
        if scheme.eq_ignore_ascii_case("Bearer") {
            evidence.add_redaction(&Secret::new(value.trim()));
        }
        if scheme.eq_ignore_ascii_case("Basic")
            && let Ok(bytes) = data_encoding::BASE64.decode(value.trim().as_bytes())
            && let Ok(decoded) = String::from_utf8(bytes)
        {
            evidence.add_redaction(&Secret::new(&decoded));
            if let Some((left, right)) = decoded.split_once(':') {
                evidence.add_redaction(&Secret::new(left));
                evidence.add_redaction(&Secret::new(right));
            }
        }
    }
}

fn visit(evidence: &mut Evidence, raw: &RawValue, depth: usize) {
    if depth == 0 || evidence.credentials().values().len() > 256 {
        return;
    }
    match raw.get().as_bytes().first() {
        Some(b'[') => {
            if let Ok(items) = serde_json::from_str::<Vec<&RawValue>>(raw.get()) {
                for item in items {
                    visit(evidence, item, depth - 1);
                }
            }
        }
        Some(b'{') => {
            if let Ok(fields) =
                serde_json::from_str::<std::collections::BTreeMap<String, &RawValue>>(raw.get())
            {
                for item in fields.into_values() {
                    visit(evidence, item, depth - 1);
                }
            }
        }
        Some(b'"') => {
            if let Ok(text) = serde_json::from_str::<String>(raw.get()) {
                add(evidence, &text);
            }
        }
        Some(b'-' | b'0'..=b'9') => evidence.add_redaction(&Secret::new(raw.get())),
        _ => (),
    }
}
