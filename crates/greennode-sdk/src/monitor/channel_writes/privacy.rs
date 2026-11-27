use super::{ChannelHeader, Evidence, Secret};
use crate::monitor::privacy::{check_budget, collect_url};
use serde_json::value::RawValue;

pub(super) fn channel_values(
    address: &str,
    headers: &[ChannelHeader],
    raw_header: &str,
    reference: &str,
    otp: &Secret,
) -> Result<Evidence, ()> {
    let mut evidence = Evidence::new();
    evidence.add_redaction(otp);
    for value in [raw_header, reference, address]
        .into_iter()
        .chain(headers.iter().map(|header| header.value.as_str()))
    {
        add(&mut evidence, value)?;
    }
    if headers.is_empty()
        && let Ok(raw) = serde_json::from_str::<&RawValue>(raw_header)
    {
        visit(&mut evidence, raw, 128)?;
    }
    Ok(evidence)
}

fn add(evidence: &mut Evidence, value: &str) -> Result<(), ()> {
    if evidence.credentials().values().len() > 256 {
        return Err(());
    }
    evidence.add_redaction(&Secret::new(value));
    collect_url(evidence, value, |_| true)?;
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
    check_budget(evidence)
}

fn visit(evidence: &mut Evidence, raw: &RawValue, depth: usize) -> Result<(), ()> {
    if depth == 0 || evidence.credentials().values().len() > 256 {
        return Err(());
    }
    match raw.get().as_bytes().first() {
        Some(b'[') => {
            for item in serde_json::from_str::<Vec<&RawValue>>(raw.get()).map_err(|_| ())? {
                visit(evidence, item, depth - 1)?;
            }
        }
        Some(b'{') => {
            for item in
                serde_json::from_str::<std::collections::BTreeMap<String, &RawValue>>(raw.get())
                    .map_err(|_| ())?
                    .into_values()
            {
                visit(evidence, item, depth - 1)?;
            }
        }
        Some(b'"') => add(
            evidence,
            &serde_json::from_str::<String>(raw.get()).map_err(|_| ())?,
        )?,
        Some(b'-' | b'0'..=b'9') => evidence.add_redaction(&Secret::new(raw.get())),
        _ => (),
    }
    Ok(())
}
