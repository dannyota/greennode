// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use crate::cli::render::Value;

use super::interpreter::{EvaluationError, WorkBudget, check};

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

pub(super) fn matches_type(value: &Value, kind: Type) -> bool {
    match kind {
        Type::Number => matches!(value, Value::Number(_)),
        Type::String => matches!(value, Value::String(_)),
        Type::Array => matches!(value, Value::Array(_)),
        Type::Object => matches!(value, Value::Object(_)),
        Type::ArrayNumber => {
            matches!(value, Value::Array(items) if items.iter().all(|item| matches!(item, Value::Number(_))))
        }
        Type::ArrayString => {
            matches!(value, Value::Array(items) if items.iter().all(|item| matches!(item, Value::String(_))))
        }
        Type::Expref => false,
        Type::Any => true,
    }
}

fn json_string(output: &mut String, value: &str) {
    output.push('"');
    for ch in value.chars() {
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
}

fn compact_json(value: &Value, output: &mut String, depth: usize) -> Result<(), EvaluationError> {
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
        Value::String(value) => json_string(output, value),
        Value::Array(values) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                compact_json(value, output, depth + 1)?;
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
                json_string(output, key);
                output.push(':');
                compact_json(value, output, depth + 1)?;
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
            if !allowed.iter().any(|kind| matches_type(argument, *kind)) {
                return Err(EvaluationError::FunctionType);
            }
        }
    }
    match (name, arguments) {
        ("keys", [Value::Object(values)]) => Ok(Value::Array(
            values.keys().cloned().map(Value::String).collect(),
        )),
        ("values", [Value::Object(values)]) => {
            for value in values.values() {
                check(value, 1)?;
            }
            Ok(Value::Array(values.values().cloned().collect()))
        }
        ("starts_with", [Value::String(value), Value::String(prefix)]) => {
            Ok(Value::Bool(value.starts_with(prefix)))
        }
        ("ends_with", [Value::String(value), Value::String(suffix)]) => {
            Ok(Value::Bool(value.ends_with(suffix)))
        }
        ("contains", [Value::String(value), needle]) => Ok(Value::Bool(
            matches!(needle, Value::String(needle) if value.contains(needle)),
        )),
        ("contains", [Value::Array(values), needle]) => {
            for value in values {
                budget.charge(1)?;
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
            let mut parts = Vec::with_capacity(values.len());
            for value in values {
                let Value::String(value) = value else {
                    return Err(EvaluationError::FunctionType);
                };
                parts.push(value.as_str());
            }
            Ok(Value::String(parts.join(separator)))
        }
        ("to_array", [Value::Array(values)]) => Ok(Value::Array(values.clone())),
        ("to_array", [value]) => {
            check(value, 1)?;
            Ok(Value::Array(vec![value.clone()]))
        }
        ("to_string", [Value::String(value)]) => Ok(Value::String(value.clone())),
        ("to_string", [value]) => {
            let mut output = String::new();
            compact_json(value, &mut output, 0)?;
            Ok(Value::String(output))
        }
        ("to_number", [Value::Number(value)]) => Ok(Value::Number(*value)),
        ("to_number", [Value::String(value)]) => match parse_go_float(value) {
            Some(value) if value.is_finite() => Ok(Value::Number(value)),
            Some(_) => Err(EvaluationError::NonFinite),
            None => Ok(Value::Null),
        },
        ("to_number", [_]) => Ok(Value::Null),
        ("sum", [Value::Array(values)]) => {
            let mut sum = 0.0;
            for value in values {
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
            let mut best = first;
            for value in &values[1..] {
                let ordering = match (value, best) {
                    (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
                    (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                    _ => return Err(EvaluationError::FunctionType),
                };
                if (name == "max" && ordering == Some(std::cmp::Ordering::Greater))
                    || (name == "min" && ordering == Some(std::cmp::Ordering::Less))
                {
                    best = value;
                }
            }
            Ok(best.clone())
        }
        ("sort", [Value::Array(values)]) => {
            if values.iter().all(|value| matches!(value, Value::Number(_))) {
                let mut numbers = values
                    .iter()
                    .map(|value| match value {
                        Value::Number(number) if number.is_finite() => Ok(*number),
                        Value::Number(_) => Err(EvaluationError::NonFinite),
                        _ => Err(EvaluationError::FunctionType),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                numbers.sort_by(|a, b| {
                    if a == b {
                        std::cmp::Ordering::Equal
                    } else {
                        a.total_cmp(b)
                    }
                });
                Ok(Value::Array(
                    numbers.into_iter().map(Value::Number).collect(),
                ))
            } else {
                let mut strings = values
                    .iter()
                    .map(|value| match value {
                        Value::String(string) => Ok(string.clone()),
                        _ => Err(EvaluationError::FunctionType),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                strings.sort();
                Ok(Value::Array(
                    strings.into_iter().map(Value::String).collect(),
                ))
            }
        }
        ("merge", values) => {
            let mut merged = std::collections::BTreeMap::new();
            for value in values {
                let Value::Object(map) = value else {
                    return Err(EvaluationError::FunctionPanic);
                };
                merged.extend(map.clone());
            }
            Ok(Value::Object(merged))
        }
        ("abs", [Value::Number(value)]) => Ok(Value::Number(value.abs())),
        ("ceil", [Value::Number(value)]) => Ok(Value::Number(value.ceil())),
        ("floor", [Value::Number(value)]) => Ok(Value::Number(value.floor())),
        ("avg", [Value::Array(values)]) => {
            let mut sum = 0.0;
            for value in values {
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
        ("type", [value]) => Ok(Value::String(
            match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            }
            .into(),
        )),
        ("not_null", values) => Ok(values
            .iter()
            .find(|value| **value != Value::Null)
            .cloned()
            .unwrap_or(Value::Null)),
        ("reverse", [Value::String(value)]) => Ok(Value::String(value.chars().rev().collect())),
        ("reverse", [Value::Array(values)]) => {
            Ok(Value::Array(values.iter().rev().cloned().collect()))
        }
        ("length", [value]) => {
            let length = match value {
                Value::String(value) => value.chars().count(),
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
