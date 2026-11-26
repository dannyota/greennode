// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use crate::cli::render::Value;

use super::json_size::json_size;

use super::interpreter::{
    EvaluationError, OBJECT_LEAF, WorkBudget, check, compare_numbers, compare_strings, stable_order,
};

#[derive(Clone, Copy)]
pub(super) enum Type {
    Number,
    String,
    Array,
    Object,
    ArrayNumber,
    ArrayString,
    Expref,
    Any,
}

pub(super) struct Signature {
    pub(super) arguments: &'static [&'static [Type]],
    pub(super) variadic: bool,
}

pub(super) fn signature(name: &str) -> Option<Signature> {
    use Type::{Any, Array, ArrayNumber, ArrayString, Expref, Number, Object, String};
    let (arguments, variadic): (&[&[Type]], bool) = match name {
        "length" => (&[&[String, Array, Object]], false),
        "starts_with" | "ends_with" => (&[&[String], &[String]], false),
        "abs" | "ceil" | "floor" => (&[&[Number]], false),
        "avg" | "sum" => (&[&[ArrayNumber]], false),
        "contains" => (&[&[Array, String], &[Any]], false),
        "map" => (&[&[Expref], &[Array]], false),
        "max" | "min" => (&[&[ArrayNumber, ArrayString]], false),
        "merge" => (&[&[Object]], true),
        "max_by" | "min_by" | "sort_by" => (&[&[Array], &[Expref]], false),
        "type" | "to_array" | "to_string" | "to_number" => (&[&[Any]], false),
        "keys" | "values" => (&[&[Object]], false),
        "sort" => (&[&[ArrayString, ArrayNumber]], false),
        "join" => (&[&[String], &[ArrayString]], false),
        "reverse" => (&[&[Array, String]], false),
        "not_null" => (&[&[Any]], true),
        _ => return None,
    };
    Some(Signature {
        arguments,
        variadic,
    })
}

pub(super) fn matches_type(
    value: &Value,
    kind: Type,
    budget: &mut WorkBudget,
) -> Result<bool, EvaluationError> {
    let matches = match kind {
        Type::Number => matches!(value, Value::Number(_)),
        Type::String => matches!(value, Value::String(_)),
        Type::Array => matches!(value, Value::Array(_)),
        Type::Object => matches!(value, Value::Object(_)),
        Type::ArrayNumber | Type::ArrayString => {
            let Value::Array(items) = value else {
                return Ok(false);
            };
            for item in items {
                budget.charge(1)?;
                if !matches!(
                    (kind, item),
                    (Type::ArrayNumber, Value::Number(_)) | (Type::ArrayString, Value::String(_))
                ) {
                    return Ok(false);
                }
            }
            true
        }
        Type::Expref => false,
        Type::Any => true,
    };
    Ok(matches)
}

fn json_string(
    output: &mut String,
    value: &str,
    budget: &mut WorkBudget,
) -> Result<(), EvaluationError> {
    output.push('"');
    for ch in value.chars() {
        budget.charge(1)?;
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                output.push_str(&format!("\\u{:04x}", u32::from(ch)))
            }
            _ => output.push(ch),
        }
    }
    output.push('"');
    Ok(())
}

fn compact_json(
    value: &Value,
    output: &mut String,
    depth: usize,
    budget: &mut WorkBudget,
) -> Result<(), EvaluationError> {
    budget.charge(1)?;
    if depth > 128 {
        return Err(EvaluationError::Depth);
    }
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(
            &crate::cli::render::query_json_float(*value)
                .map_err(|_| EvaluationError::NonFinite)?,
        ),
        Value::String(value) => json_string(output, value, budget)?,
        Value::Array(values) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                compact_json(value, output, depth + 1, budget)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            output.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                json_string(output, key, budget)?;
                output.push(':');
                compact_json(value, output, depth + 1, budget)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn valid_digits(value: &str, radix: u32) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().all(|(i, byte)| {
        if *byte == b'_' {
            i > 0
                && i + 1 < bytes.len()
                && (bytes[i - 1] as char).is_digit(radix)
                && (bytes[i + 1] as char).is_digit(radix)
        } else {
            (*byte as char).is_digit(radix)
        }
    })
}

fn parse_go_float(value: &str) -> Option<f64> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "nan" => return Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let negative = lower.starts_with('-');
    let unsigned = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    let (hex, number) = match unsigned.strip_prefix("0x") {
        Some(rest) => (true, rest),
        None => (false, unsigned),
    };
    let marker = if hex { 'p' } else { 'e' };
    let (significand, exponent_text) = match number.split_once(marker) {
        Some((a, b)) => (a, Some(b)),
        None if hex => return None,
        None => (number, None),
    };
    let exponent = if let Some(text) = exponent_text {
        let neg = text.starts_with('-');
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        if digits.is_empty() || !valid_digits(digits, 10) {
            return None;
        }
        let clean = digits.replace('_', "");
        let significant = clean.trim_start_matches('0');
        let prefix = &significant[..significant.len().min(5)];
        let magnitude = if prefix.is_empty() {
            0
        } else {
            prefix.parse::<i64>().ok()?
        };
        if neg { -magnitude } else { magnitude }
    } else {
        0
    };
    let significand = if hex && significand.starts_with('_') {
        let rest = &significand[1..];
        if !rest.as_bytes().first().is_some_and(u8::is_ascii_hexdigit) {
            return None;
        }
        rest
    } else {
        significand
    };
    let (integer, fraction) = significand.split_once('.').unwrap_or((significand, ""));
    let radix = if hex { 16 } else { 10 };
    if (integer.is_empty() && fraction.is_empty())
        || !valid_digits(integer, radix)
        || !valid_digits(fraction, radix)
    {
        return None;
    }
    if !hex {
        let clean = format!(
            "{}{}e{exponent}",
            if negative { "-" } else { "" },
            significand.replace('_', "")
        );
        let parsed = clean.parse::<f64>().ok()?;
        return parsed.is_finite().then_some(parsed);
    }
    let integer = integer.replace('_', "");
    let fraction = fraction.replace('_', "");
    let point = i64::try_from(integer.len()).ok()?;
    let digits = integer + &fraction;
    let trimmed = digits.trim_start_matches('0');
    let leading = i64::try_from(digits.len() - trimmed.len()).ok()?;
    let kept = trimmed.len().min(16);
    let mantissa = if kept == 0 {
        0
    } else {
        u64::from_str_radix(&trimmed[..kept], 16).ok()?
    };
    let truncated = trimmed[kept..].bytes().any(|b| b != b'0');
    let scale = exponent.saturating_add(4 * (point - leading - i64::try_from(kept).ok()?));
    let sign = u64::from(negative) << 63;
    if mantissa == 0 {
        return Some(f64::from_bits(sign));
    }
    let high = i64::from(63 - mantissa.leading_zeros());
    let mut power = high.saturating_add(scale);
    if power > 1023 {
        return None;
    }
    let shift = if power >= -1022 {
        high - 52
    } else {
        (-1074i64).saturating_sub(scale)
    };
    let rounded = if shift <= 0 {
        mantissa << u32::try_from(-shift).ok()?
    } else if shift > 64 {
        0
    } else {
        let shift = u32::try_from(shift).ok()?;
        let retained = if shift == 64 { 0 } else { mantissa >> shift };
        let halfway = 1u64 << (shift - 1);
        let remainder = if shift == 64 {
            mantissa
        } else {
            mantissa & ((1u64 << shift) - 1)
        };
        retained
            + u64::from(
                remainder > halfway || (remainder == halfway && (truncated || retained & 1 != 0)),
            )
    };
    let bits = if power < -1022 {
        rounded
    } else {
        let mut significand = rounded;
        if significand == 1 << 53 {
            significand >>= 1;
            power += 1;
        }
        if power > 1023 {
            return None;
        }
        (((power + 1023) as u64) << 52) | (significand & ((1 << 52) - 1))
    };
    Some(f64::from_bits(sign | bits))
}

fn bytes_equal(
    left: &[u8],
    right: &[u8],
    budget: &mut WorkBudget,
) -> Result<bool, EvaluationError> {
    for (a, b) in left.iter().zip(right) {
        budget.charge(1)?;
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
fn call(name: &str, arguments: &[Value]) -> Result<Value, EvaluationError> {
    call_budgeted(name, arguments, &mut WorkBudget::new())
}

pub(super) fn call_budgeted(
    name: &str,
    arguments: &[Value],
    budget: &mut WorkBudget,
) -> Result<Value, EvaluationError> {
    let signature = signature(name).ok_or(EvaluationError::UnknownFunction)?;
    if signature.variadic {
        if arguments.len() < signature.arguments.len() {
            return Err(EvaluationError::FunctionVariadicArity);
        }
    } else {
        if arguments.len() != signature.arguments.len() {
            return Err(EvaluationError::FunctionArity);
        }
        for (argument, allowed) in arguments.iter().zip(signature.arguments) {
            let mut valid = false;
            for kind in *allowed {
                if matches_type(argument, *kind, budget)? {
                    valid = true;
                    break;
                }
            }
            if !valid {
                return Err(EvaluationError::FunctionType);
            }
        }
    }
    match (name, arguments) {
        ("keys", [Value::Object(values)]) => {
            budget.charge(1)?;
            let mut result = Vec::new();
            for key in values.keys() {
                budget.charge(2)?;
                budget.charge_bytes(key)?;
                budget.allocate(key.len())?;
                budget.reserve(&mut result)?;
                result.push(Value::String(key.clone()));
            }
            Ok(Value::Array(result))
        }
        ("values", [Value::Object(values)]) => {
            budget.charge(1)?;
            let mut result = Vec::new();
            for value in values.values() {
                budget.charge(1)?;
                check(value, 1, budget)?;
                budget.charge_value(value)?;
                budget.reserve(&mut result)?;
                result.push(value.clone());
            }
            Ok(Value::Array(result))
        }
        ("starts_with", [Value::String(value), Value::String(prefix)]) => {
            if prefix.len() > value.len() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(bytes_equal(
                &value.as_bytes()[..prefix.len()],
                prefix.as_bytes(),
                budget,
            )?))
        }
        ("ends_with", [Value::String(value), Value::String(suffix)]) => {
            if suffix.len() > value.len() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(bytes_equal(
                &value.as_bytes()[value.len() - suffix.len()..],
                suffix.as_bytes(),
                budget,
            )?))
        }
        ("contains", [Value::String(value), Value::String(needle)]) => {
            if needle.len() > value.len() {
                return Ok(Value::Bool(false));
            }
            let bytes = value
                .len()
                .checked_add(needle.len())
                .ok_or(EvaluationError::WorkBudget)?;
            budget.charge(u64::try_from(bytes).map_err(|_| EvaluationError::WorkBudget)?)?;
            Ok(Value::Bool(value.contains(needle)))
        }
        ("contains", [Value::String(_), _]) => Ok(Value::Bool(false)),
        ("contains", [Value::Array(values), needle]) => {
            for value in values {
                budget.charge(1)?;
                if let (Value::String(left), Value::String(right)) = (value, needle) {
                    budget.charge_compare_bytes(left, right)?;
                }
                if matches!(
                    (value, needle),
                    (Value::Array(_), Value::Array(_)) | (Value::Object(_), Value::Object(_))
                ) {
                    return Err(EvaluationError::Uncomparable);
                }
                if value == needle {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        ("join", [Value::String(separator), Value::Array(values)]) => {
            let mut parts = Vec::new();
            let mut bytes = 0usize;
            for value in values {
                budget.charge(1)?;
                let Value::String(value) = value else {
                    return Err(EvaluationError::FunctionType);
                };
                if !parts.is_empty() {
                    bytes = bytes
                        .checked_add(separator.len())
                        .ok_or(EvaluationError::WorkBudget)?;
                }
                bytes = bytes
                    .checked_add(value.len())
                    .ok_or(EvaluationError::WorkBudget)?;
                budget.push(&mut parts, value.as_str())?;
            }
            budget.allocate(bytes)?;
            budget.charge(u64::try_from(bytes).map_err(|_| EvaluationError::WorkBudget)?)?;
            Ok(Value::String(parts.join(separator)))
        }
        ("to_array", [Value::Array(values)]) => {
            budget.allocate(
                values
                    .len()
                    .checked_mul(std::mem::size_of::<Value>())
                    .ok_or(EvaluationError::ResultSize)?,
            )?;
            budget.charge(1)?;
            for value in values {
                budget.charge_value(value)?;
            }
            Ok(Value::Array(values.clone()))
        }
        ("to_array", [value]) => {
            budget.allocate(std::mem::size_of::<Value>())?;
            budget.charge(1)?;
            check(value, 1, budget)?;
            budget.charge_value(value)?;
            Ok(Value::Array(vec![value.clone()]))
        }
        ("to_string", [Value::String(value)]) => {
            budget.allocate(value.len())?;
            budget.charge(u64::try_from(value.len()).map_err(|_| EvaluationError::WorkBudget)?)?;
            Ok(Value::String(value.clone()))
        }
        ("to_string", [value]) => {
            let size = json_size(value, budget, 0)?;
            budget.allocate(size)?;
            budget.charge(u64::try_from(size).map_err(|_| EvaluationError::WorkBudget)?)?;
            let mut output = String::with_capacity(size);
            compact_json(value, &mut output, 0, budget)?;
            Ok(Value::String(output))
        }
        ("to_number", [Value::Number(value)]) => Ok(Value::Number(*value)),
        ("to_number", [Value::String(value)]) => {
            budget.charge(u64::try_from(value.len()).map_err(|_| EvaluationError::WorkBudget)?)?;
            match parse_go_float(value) {
                Some(value) if value.is_finite() => Ok(Value::Number(value)),
                Some(_) => Err(EvaluationError::NonFinite),
                None => Ok(Value::Null),
            }
        }
        ("to_number", [_]) => Ok(Value::Null),
        ("sum", [Value::Array(values)]) => {
            let mut sum = 0.0;
            for value in values {
                budget.charge(1)?;
                let Value::Number(number) = value else {
                    return Err(EvaluationError::FunctionType);
                };
                sum += number;
            }
            if sum.is_finite() {
                Ok(Value::Number(sum))
            } else {
                Err(EvaluationError::NonFinite)
            }
        }
        ("max" | "min", [Value::Array(values)]) => {
            let Some(first) = values.first() else {
                return Ok(Value::Null);
            };
            budget.charge(1)?;
            let mut best = first;
            for value in &values[1..] {
                budget.charge(1)?;
                let ordering = match (value, best) {
                    (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
                    (Value::String(a), Value::String(b)) => {
                        budget.charge_compare_bytes(a, b)?;
                        Some(a.cmp(b))
                    }
                    _ => return Err(EvaluationError::FunctionType),
                };
                if (name == "max" && ordering == Some(std::cmp::Ordering::Greater))
                    || (name == "min" && ordering == Some(std::cmp::Ordering::Less))
                {
                    best = value;
                }
            }
            budget.charge_value(best)?;
            Ok(best.clone())
        }
        ("sort", [Value::Array(values)]) => {
            budget.charge(1)?;
            for value in values {
                budget.charge(1)?;
                if matches!(value, Value::Number(number) if !number.is_finite()) {
                    return Err(EvaluationError::NonFinite);
                }
            }
            let order = stable_order(values, budget, |a, b, budget| match (a, b) {
                (Value::Number(a), Value::Number(b)) => compare_numbers(a, b, budget),
                (Value::String(a), Value::String(b)) => compare_strings(a, b, budget),
                _ => Err(EvaluationError::FunctionType),
            })?;
            let mut result = Vec::new();
            for index in order {
                budget.charge_value(&values[index])?;
                budget.reserve(&mut result)?;
                result.push(values[index].clone());
            }
            Ok(Value::Array(result))
        }
        ("merge", values) => {
            budget.charge(1)?;
            budget.allocate(OBJECT_LEAF)?;
            let mut merged = std::collections::BTreeMap::new();
            for value in values {
                let Value::Object(map) = value else {
                    return Err(EvaluationError::FunctionPanic);
                };
                for (key, value) in map {
                    budget.charge(1)?;
                    budget.charge_bytes(key)?;
                    budget.charge_value(value)?;
                    budget.charge_merge_key(key, merged.len())?;
                    budget.allocate(
                        key.len()
                            .checked_add(if merged.len() >= 11 { 128 } else { 0 })
                            .ok_or(EvaluationError::ResultSize)?,
                    )?;
                    merged.insert(key.clone(), value.clone());
                }
            }
            Ok(Value::Object(merged))
        }
        ("abs", [Value::Number(value)]) => Ok(Value::Number(value.abs())),
        ("ceil", [Value::Number(value)]) => Ok(Value::Number(value.ceil())),
        ("floor", [Value::Number(value)]) => Ok(Value::Number(value.floor())),
        ("avg", [Value::Array(values)]) => {
            let mut sum = 0.0;
            for value in values {
                budget.charge(1)?;
                let Value::Number(value) = value else {
                    return Err(EvaluationError::FunctionType);
                };
                sum += value;
            }
            let average = sum / values.len() as f64;
            if average.is_finite() {
                Ok(Value::Number(average))
            } else {
                Err(EvaluationError::NonFinite)
            }
        }
        ("type", [value]) => {
            let result = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            budget.allocate(result.len())?;
            Ok(Value::String(result.into()))
        }
        ("not_null", values) => {
            for value in values {
                budget.charge(1)?;
                if *value != Value::Null {
                    budget.charge_value(value)?;
                    return Ok(value.clone());
                }
            }
            Ok(Value::Null)
        }
        ("reverse", [Value::String(value)]) => {
            budget.allocate(value.len())?;
            for _ in value.chars() {
                budget.charge(1)?;
            }
            budget.charge(u64::try_from(value.len()).map_err(|_| EvaluationError::WorkBudget)?)?;
            let mut result = String::with_capacity(value.len());
            result.extend(value.chars().rev());
            Ok(Value::String(result))
        }
        ("reverse", [Value::Array(values)]) => {
            budget.charge(1)?;
            let mut result = Vec::new();
            for value in values.iter().rev() {
                budget.charge(1)?;
                budget.charge_value(value)?;
                budget.reserve(&mut result)?;
                result.push(value.clone());
            }
            Ok(Value::Array(result))
        }
        ("length", [value]) => {
            let length = match value {
                Value::String(value) => {
                    let mut length = 0;
                    for _ in value.chars() {
                        budget.charge(1)?;
                        length += 1;
                    }
                    length
                }
                Value::Array(value) => value.len(),
                Value::Object(value) => value.len(),
                _ => return Err(EvaluationError::FunctionType),
            };
            Ok(Value::Number(length as f64))
        }
        _ => Err(EvaluationError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn to_string_charges_large_output_against_remaining_allocation_budget() {
        use super::super::interpreter::{RESULT_LIMIT, unlimited_budget};
        let input = Value::String("x".repeat(1024 * 1024));
        let mut budget = unlimited_budget();
        budget.allocate(RESULT_LIMIT - 1024 * 1024 + 1).unwrap();
        assert_eq!(
            call_budgeted("to_string", std::slice::from_ref(&input), &mut budget),
            Err(EvaluationError::ResultSize)
        );
        let mut budget = unlimited_budget();
        budget.allocate(RESULT_LIMIT - 1024 * 1024).unwrap();
        assert_eq!(
            call_budgeted("to_string", std::slice::from_ref(&input), &mut budget),
            Ok(input.clone())
        );
    }

    #[test]
    fn function_results_stop_at_size_limit() {
        use super::super::interpreter::{RESULT_LIMIT, unlimited_budget};
        let overhead = std::mem::size_of::<Value>();
        let text = "x".repeat(RESULT_LIMIT - 2 * overhead + 1);
        let input = Value::String(text);
        assert_eq!(
            call_budgeted(
                "to_array",
                std::slice::from_ref(&input),
                &mut unlimited_budget()
            ),
            Err(EvaluationError::ResultSize)
        );
        let object = Value::Object(BTreeMap::from([("k".into(), input)]));
        assert_eq!(
            call_budgeted(
                "values",
                std::slice::from_ref(&object),
                &mut unlimited_budget()
            ),
            Err(EvaluationError::ResultSize)
        );
        assert_eq!(
            call_budgeted(
                "merge",
                std::slice::from_ref(&object),
                &mut unlimited_budget()
            ),
            Err(EvaluationError::ResultSize)
        );
        let args = [
            Value::String("x".repeat(100)),
            Value::Array(vec![
                Value::String("a".repeat(RESULT_LIMIT - 100)),
                Value::String("b".repeat(100)),
            ]),
        ];
        assert_eq!(
            call_budgeted("join", &args, &mut unlimited_budget()),
            Err(EvaluationError::ResultSize)
        );
    }

    #[test]
    fn merge_charges_key_comparisons() {
        let prefix = "x".repeat(1_024);
        let left = Value::Object(BTreeMap::from([(format!("{prefix}a"), Value::Null)]));
        let right = Value::Object(BTreeMap::from([(format!("{prefix}b"), Value::Null)]));
        let mut budget = WorkBudget::new();
        budget.charge(9_997_500).unwrap();
        assert_eq!(
            call_budgeted("merge", &[left, right], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn contains_long_repeated_strings_uses_linear_work() {
        let input = Value::String("a".repeat(1024 * 1024));
        let needle = Value::String(format!("{}b", "a".repeat(32 * 1024)));
        let mut budget = WorkBudget::new();
        budget.charge(8_000_000).unwrap();
        assert_eq!(
            call_budgeted("contains", &[input, needle], &mut budget),
            Ok(Value::Bool(false))
        );
    }

    #[test]
    fn extrema_and_sort_charge_string_comparison_bytes() {
        let prefix = "x".repeat(1_024);
        let input = Value::Array(vec![
            Value::String(format!("{prefix}a")),
            Value::String(format!("{prefix}b")),
        ]);
        for (name, used) in [("min", 9_998_500), ("max", 9_998_500), ("sort", 9_997_500)] {
            let mut budget = WorkBudget::new();
            budget.charge(used).unwrap();
            assert_eq!(
                call_budgeted(name, std::slice::from_ref(&input), &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn contains_charges_long_string_comparisons() {
        let prefix = "x".repeat(64 * 1024);
        let haystack = Value::Array(vec![Value::String(format!("{prefix}a")); 64]);
        let needle = Value::String(format!("{prefix}b"));
        let args = [haystack, needle];
        let mut budget = WorkBudget::new();
        assert_eq!(
            call_budgeted("contains", &args, &mut budget),
            Ok(Value::Bool(false))
        );
        assert_eq!(
            call_budgeted("contains", &args, &mut budget),
            Ok(Value::Bool(false))
        );
        assert_eq!(
            call_budgeted("contains", &args, &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn sorted_strings_do_not_pay_worst_case_sort_budget() {
        let prefix = "x".repeat(256);
        let input = Value::Array(
            (0..999)
                .map(|n| Value::String(format!("{prefix}{n:04}")))
                .collect(),
        );
        assert!(matches!(call("sort", &[input]), Ok(Value::Array(values)) if values.len() == 999));
    }

    #[test]
    fn descending_string_sort_stops_at_budget() {
        let prefix = "x".repeat(80);
        let input = Value::Array(
            (0..80_000)
                .rev()
                .map(|n| Value::String(format!("{prefix}{n:05}")))
                .collect(),
        );
        assert!(matches!(
            call("sort", &[input]),
            Err(EvaluationError::WorkBudget)
        ));
    }

    #[test]
    fn merge_two_thousand_keys_stays_within_budget() {
        let input = Value::Object(
            (0..2_000)
                .map(|n| (format!("key{n:05}"), Value::Null))
                .collect(),
        );
        assert!(
            matches!(call("merge", &[input]), Ok(Value::Object(values)) if values.len() == 2_000)
        );
    }

    #[test]
    fn standalone_key_copies_consume_byte_budget() {
        for name in ["keys", "merge"] {
            let input = Value::Object(BTreeMap::from([("x".repeat(1_024), Value::Null)]));
            let mut budget = WorkBudget::new();
            budget.charge(9_999_500).unwrap();
            assert_eq!(
                call_budgeted(name, &[input], &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn exhausted_sort_comparison_returns_error() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_999).unwrap();
        assert_eq!(
            compare_strings("a", "b", &mut budget),
            Err(EvaluationError::WorkBudget)
        );
        assert_eq!(
            compare_numbers(&1.0, &2.0, &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn sort_comparisons_count_comparisons_and_bytes() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_996).unwrap();
        assert_eq!(
            compare_numbers(&1.0, &2.0, &mut budget),
            Ok(std::cmp::Ordering::Less)
        );
        assert_eq!(
            compare_strings("ab", "ab", &mut budget),
            Ok(std::cmp::Ordering::Equal)
        );
        assert_eq!(budget.charge(1), Err(EvaluationError::WorkBudget));
    }

    #[test]
    fn number_parsing_charges_input_bytes() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_998).unwrap();
        assert_eq!(
            call_budgeted("to_number", &[Value::String("123".into())], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn empty_function_output_charges_container() {
        let mut budget = WorkBudget::new();
        budget.charge(10_000_000).unwrap();
        assert_eq!(
            call_budgeted("keys", &[Value::Object(BTreeMap::new())], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn string_searches_charge_examined_bytes() {
        for (name, value, needle) in [
            ("starts_with", "abc", "abc"),
            ("ends_with", "abc", "abc"),
            ("contains", "abc", "bc"),
        ] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_999).unwrap();
            assert_eq!(
                call_budgeted(
                    name,
                    &[Value::String(value.into()), Value::String(needle.into())],
                    &mut budget
                ),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn reverse_strings_and_not_null_charge_scans() {
        let mut nulls = vec![Value::Null; 1_000];
        nulls.push(Value::Number(1.0));
        for (name, args) in [
            ("reverse", vec![Value::String("a".repeat(1_000))]),
            ("not_null", nulls),
        ] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_500).unwrap();
            assert_eq!(
                call_budgeted(name, &args, &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn sort_charges_comparisons() {
        let mut budget = WorkBudget::new();
        budget.charge(9_996_000).unwrap();
        let input = Value::Array((0..1_024).rev().map(|n| Value::Number(n as f64)).collect());
        assert_eq!(
            call_budgeted("sort", &[input], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn array_type_checks_charge_examined_elements() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_998).unwrap();
        let input = Value::Array(vec![
            Value::Number(1.0),
            Value::Number(2.0),
            Value::String("x".into()),
        ]);
        assert_eq!(
            call_budgeted("avg", &[input], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn to_string_charges_emission_scan() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_988).unwrap();
        let input = Value::Array(vec![Value::String("abc".into())]);
        assert_eq!(
            call_budgeted("to_string", &[input], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn to_string_charges_output_bytes() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_998).unwrap();
        assert_eq!(
            call_budgeted("to_string", &[Value::Number(1.0)], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn string_outputs_are_charged_before_allocation() {
        for (name, arguments) in [
            (
                "join",
                vec![
                    Value::String("-".into()),
                    Value::Array(vec![Value::String("abc".into())]),
                ],
            ),
            (
                "to_string",
                vec![Value::Array(vec![Value::String("abc".into())])],
            ),
        ] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_998).unwrap();
            assert_eq!(
                call_budgeted(name, &arguments, &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn aggregate_scans_charge_elements() {
        for name in ["sum", "avg", "min", "max"] {
            let mut budget = WorkBudget::new();
            budget.charge(9_998_501).unwrap();
            let input = Value::Array(vec![Value::Number(1.0); 1_000]);
            assert_eq!(
                call_budgeted(name, &[input], &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn object_output_charges_elements() {
        for name in ["keys", "values", "merge"] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_500).unwrap();
            let input = Value::Object(
                (0..1_000)
                    .map(|index| (index.to_string(), Value::Null))
                    .collect(),
            );
            assert_eq!(
                call_budgeted(name, &[input], &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn reverse_and_to_array_charge_produced_values() {
        for name in ["reverse", "to_array"] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_500).unwrap();
            let input = Value::Array(vec![Value::Null; 1_000]);
            assert_eq!(
                call_budgeted(name, &[input], &mut budget),
                Err(EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn string_length_charges_characters() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_500).unwrap();
        assert_eq!(
            call_budgeted("length", &[Value::String("🌿".repeat(1_000))], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn contains_charges_each_scanned_element() {
        let mut budget = WorkBudget::new();
        budget.charge(9_999_998).unwrap();
        let haystack = Value::Array(vec![Value::Number(0.0); 3]);
        assert_eq!(
            call_budgeted("contains", &[haystack, Value::Number(1.0)], &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn sort_rejects_nonfinite_numbers() {
        assert_eq!(
            call(
                "sort",
                &[Value::Array(vec![
                    Value::Number(1.0),
                    Value::Number(f64::NAN),
                    Value::Number(2.0)
                ])]
            ),
            Err(EvaluationError::NonFinite)
        );
    }

    #[test]
    fn aggregate_functions_follow_go() {
        let arr = |v: Vec<Value>| Value::Array(v);
        let n = Value::Number;
        assert_eq!(
            call("sum", &[arr(vec![n(1e16), n(1.0), n(-1e16)])]),
            Ok(n(0.0))
        );
        assert_eq!(call("sum", &[arr(vec![n(1.0), n(2.0)])]), Ok(n(3.0)));
        assert_eq!(call("sum", &[arr(vec![])]), Ok(n(0.0)));
        assert_eq!(
            call("sum", &[arr(vec![n(1.0), Value::String("a".into())])]),
            Err(EvaluationError::FunctionType)
        );
        assert_eq!(call("max", &[arr(vec![])]), Ok(Value::Null));
        assert_eq!(call("min", &[arr(vec![n(3.0), n(1.0)])]), Ok(n(1.0)));
        assert_eq!(
            call(
                "max",
                &[arr(vec![
                    Value::String("é".into()),
                    Value::String("z".into())
                ])]
            ),
            Ok(Value::String("é".into()))
        );
        assert_eq!(
            call(
                "sort",
                &[arr(vec![
                    Value::String("é".into()),
                    Value::String("z".into()),
                    Value::String("中".into())
                ])]
            ),
            Ok(arr(vec![
                Value::String("z".into()),
                Value::String("é".into()),
                Value::String("中".into())
            ]))
        );
        assert_eq!(
            call("sort", &[arr(vec![n(1.0), Value::String("a".into())])]),
            Err(EvaluationError::FunctionType)
        );
        let a = Value::Object(BTreeMap::from([("x".into(), n(1.0))]));
        let b = Value::Object(BTreeMap::from([("x".into(), n(2.0))]));
        assert_eq!(call("merge", &[a, b.clone()]), Ok(b));
        assert_eq!(
            call("merge", &[n(1.0)]),
            Err(EvaluationError::FunctionPanic)
        );
    }

    #[test]
    fn to_number_limits_exponent_accumulation_like_go() {
        let hex = format!("0x1{}p-100000", "0".repeat(2500));
        let decimal = format!("0.{}1e100000", "0".repeat(9999));
        for text in [hex, decimal] {
            assert_eq!(
                call("to_number", &[Value::String(text)]),
                Ok(Value::Number(1.0))
            );
        }
    }

    #[test]
    fn to_number_matches_go_float_cases() {
        for (text, expected) in [
            ("1_2", Some(12.0)),
            ("_12", None),
            ("12_", None),
            ("1__2", None),
            ("1_.2", None),
            ("true", None),
            ("0x1.8p+1", Some(3.0)),
            ("0x1.00000000000008p0", Some(1.0)),
            (
                "0x1.0000000000000801p0",
                Some(f64::from_bits(1.0f64.to_bits() + 1)),
            ),
            ("0x1.fffffffffffffp1023", Some(f64::MAX)),
            ("0x1p1024", None),
            ("0x1p-1074", Some(f64::from_bits(1))),
            ("0x1p-1075", Some(0.0)),
            ("-NaN", None),
            ("1e999", None),
        ] {
            let actual = call("to_number", &[Value::String(text.into())]);
            assert_eq!(
                actual,
                Ok(expected.map_or(Value::Null, Value::Number)),
                "{text}"
            );
        }
        for text in ["NaN", "+Inf", "-infinity"] {
            assert_eq!(
                call("to_number", &[Value::String(text.into())]),
                Err(EvaluationError::NonFinite)
            );
        }
    }

    #[test]
    fn to_string_uses_compact_go_json() {
        let input = Value::Array(vec![
            Value::Number(1e-7),
            Value::String("<&\u{007f}\u{2028}".into()),
        ]);
        assert_eq!(
            call("to_string", &[input]),
            Ok(Value::String(
                "[1e-7,\"\\u003c\\u0026\u{007f}\\u2028\"]".into()
            ))
        );
        assert_eq!(
            call("to_string", &[Value::String("<&".into())]),
            Ok(Value::String("<&".into()))
        );
        assert_eq!(
            call(
                "to_string",
                &[Value::Array(vec![Value::String("\u{8}\u{c}".into())])]
            ),
            Ok(Value::String("[\"\\b\\f\"]".into()))
        );
    }

    #[test]
    fn object_members_follow_utf8_order() {
        let object = Value::Object(BTreeMap::from([
            ("é".into(), Value::Number(2.0)),
            ("z".into(), Value::Number(1.0)),
        ]));
        assert_eq!(
            call("keys", std::slice::from_ref(&object)),
            Ok(Value::Array(vec![
                Value::String("z".into()),
                Value::String("é".into())
            ]))
        );
        assert_eq!(
            call("values", &[object]),
            Ok(Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]))
        );
    }

    #[test]
    fn string_operations_and_arrays() {
        let string = Value::String("héllo".into());
        assert_eq!(
            call("starts_with", &[string.clone(), Value::String("hé".into())]),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            call("ends_with", &[string.clone(), Value::String("llo".into())]),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            call("ends_with", &[string.clone(), Value::String("x".into())]),
            Ok(Value::Bool(false))
        );
        assert_eq!(
            call(
                "join",
                &[
                    Value::String("-".into()),
                    Value::Array(vec![string.clone(), Value::String("x".into())])
                ]
            ),
            Ok(Value::String("héllo-x".into()))
        );
        assert_eq!(
            call("to_array", std::slice::from_ref(&string)),
            Ok(Value::Array(vec![string]))
        );
        assert_eq!(
            call("to_array", &[Value::Array(vec![])]),
            Ok(Value::Array(vec![]))
        );
    }

    #[test]
    fn contains_uses_go_interface_comparison() {
        assert_eq!(
            call(
                "contains",
                &[Value::Array(vec![Value::Number(1.0)]), Value::Number(1.0)]
            ),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            call(
                "contains",
                &[
                    Value::Array(vec![Value::Array(vec![])]),
                    Value::Object(BTreeMap::new())
                ]
            ),
            Ok(Value::Bool(false))
        );
        assert_eq!(
            call(
                "contains",
                &[
                    Value::Array(vec![Value::Array(vec![])]),
                    Value::Array(vec![])
                ]
            ),
            Err(EvaluationError::Uncomparable)
        );
        assert_eq!(
            call(
                "contains",
                &[
                    Value::Array(vec![Value::Object(BTreeMap::new())]),
                    Value::Object(BTreeMap::new())
                ]
            ),
            Err(EvaluationError::Uncomparable)
        );
        assert_eq!(
            call(
                "contains",
                &[Value::String("hello".into()), Value::Number(1.0)]
            ),
            Ok(Value::Bool(false))
        );
    }
}
