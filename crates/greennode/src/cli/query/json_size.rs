// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use crate::cli::render::Value;

use super::interpreter::{EvaluationError, WorkBudget};

fn json_string_size(value: &str, budget: &mut WorkBudget) -> Result<usize, EvaluationError> {
    let mut size = 2usize;
    for ch in value.chars() {
        budget.charge(1)?;
        let bytes = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => 6,
            _ => ch.len_utf8(),
        };
        size = size.checked_add(bytes).ok_or(EvaluationError::WorkBudget)?;
    }
    Ok(size)
}

pub(super) fn json_size(
    value: &Value,
    budget: &mut WorkBudget,
    depth: usize,
) -> Result<usize, EvaluationError> {
    if depth > 128 {
        return Err(EvaluationError::Depth);
    }
    budget.charge(1)?;
    match value {
        Value::Null => Ok(4),
        Value::Bool(value) => Ok(if *value { 4 } else { 5 }),
        Value::Number(value) => Ok(crate::cli::render::query_json_float(*value)
            .map_err(|_| EvaluationError::NonFinite)?
            .len()),
        Value::String(value) => json_string_size(value, budget),
        Value::Array(values) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            let mut size = 2usize;
            for (index, value) in values.iter().enumerate() {
                let element_size = json_size(value, budget, depth + 1)?;
                size = size
                    .checked_add(usize::from(index > 0))
                    .and_then(|n| n.checked_add(element_size))
                    .ok_or(EvaluationError::WorkBudget)?;
            }
            Ok(size)
        }
        Value::Object(values) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            let mut size = 2usize;
            for (index, (key, value)) in values.iter().enumerate() {
                let key_size = json_string_size(key, budget)?;
                let value_size = json_size(value, budget, depth + 1)?;
                size = size
                    .checked_add(usize::from(index > 0) + 1)
                    .and_then(|n| n.checked_add(key_size))
                    .and_then(|n| n.checked_add(value_size))
                    .ok_or(EvaluationError::WorkBudget)?;
            }
            Ok(size)
        }
    }
}
