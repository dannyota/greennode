use serde::Serialize;
use serde_json::Value;

use crate::wire::product::evidence::Evidence;

pub(super) type IdentifierPath = &'static [&'static str];

pub(super) trait RequestModel: Serialize {
    const IDENTIFIER_PATHS: &'static [IdentifierPath];
}

pub(super) fn reflects_identifier(
    value: &Value,
    path: IdentifierPath,
    evidence: &Evidence,
) -> bool {
    let Some((field, rest)) = path.split_first() else {
        return evidence.identifier_output_reflects(value);
    };
    if let Value::Array(items) = value {
        return items
            .iter()
            .any(|item| reflects_identifier(item, path, evidence));
    }
    value
        .get(field)
        .is_none_or(|member| reflects_identifier(member, rest, evidence))
}
