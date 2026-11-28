use super::*;

pub(super) fn busy(progress: &str, parent: bool) -> bool {
    matches!(progress, "CREATING" | "UPDATING" | "DELETING")
        || (parent && progress == "CREATING-BILLING")
}

pub(super) fn message_has_phrase(message: &str, phrases: &[&[u8]]) -> bool {
    phrases.iter().any(|phrase| {
        message
            .as_bytes()
            .windows(phrase.len())
            .any(|part| part.eq_ignore_ascii_case(phrase))
    })
}

pub(super) fn canonical_field<T: WireField>(
    fields: &Envelope<'_>,
    read: &Read,
    evidence: &Evidence,
    name: &str,
) -> Result<T, Box<Error>> {
    read.parse(fields.canonical_raw(read, evidence, name)?, evidence)
}
