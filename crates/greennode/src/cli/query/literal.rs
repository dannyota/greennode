use crate::cli::render::{Value, decode_string};

use super::lexer::SyntaxError;

pub(super) fn parse(input: &str, offset: usize) -> Result<Value, SyntaxError> {
    let error = || SyntaxError { offset };
    let mut normalized = String::with_capacity(input.len());
    let mut chars = input.char_indices();
    let mut depth = 0_usize;
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => {
                let mut escaped = false;
                let mut end = None;
                for (position, next) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if next == '\\' {
                        escaped = true;
                    } else if next == '"' {
                        end = Some(position + 1);
                        break;
                    }
                }
                let quoted = input.get(index..end.ok_or_else(error)?).ok_or_else(error)?;
                let decoded = decode_string(quoted).map_err(|_| error())?;
                normalized.push_str(&serde_json::to_string(&decoded).map_err(|_| error())?);
            }
            '[' | '{' => {
                if depth >= 64 {
                    return Err(error());
                }
                depth += 1;
                normalized.push(ch);
            }
            ']' | '}' => {
                depth = depth.checked_sub(1).ok_or_else(error)?;
                normalized.push(ch);
            }
            _ => normalized.push(ch),
        }
    }
    if depth != 0 {
        return Err(error());
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&normalized).map_err(|_| error())?;
    convert(parsed).ok_or_else(error)
}

fn convert(value: serde_json::Value) -> Option<Value> {
    match value {
        serde_json::Value::Null => Some(Value::Null),
        serde_json::Value::Bool(value) => Some(Value::Bool(value)),
        serde_json::Value::String(value) => Some(Value::String(value)),
        serde_json::Value::Number(value) => {
            value.as_f64().filter(|n| n.is_finite()).map(Value::Number)
        }
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(convert)
            .collect::<Option<_>>()
            .map(Value::Array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Some((key, convert(value)?)))
            .collect::<Option<_>>()
            .map(Value::Object),
    }
}
