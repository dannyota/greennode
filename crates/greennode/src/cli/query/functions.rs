// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use crate::cli::render::Value;

use super::interpreter::EvaluationError;

#[derive(Clone, Copy)]
enum Type {
    Number,
    String,
    Array,
    Object,
    ArrayNumber,
    ArrayString,
    Expref,
    Any,
}

struct Signature {
    arguments: &'static [&'static [Type]],
    variadic: bool,
}

fn signature(name: &str) -> Option<Signature> {
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

fn matches_type(value: &Value, kind: Type) -> bool {
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

fn parse_go_float(value: &str) -> Option<f64> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "nan" => return Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let bytes = value.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let negative = bytes.first() == Some(&b'-');
    let hex = bytes
        .get(index..index + 2)
        .is_some_and(|s| s.eq_ignore_ascii_case(b"0x"));
    if hex {
        index += 2;
    }
    let mut last_digit = hex;
    let mut pending_underscore = false;
    let mut digits = false;
    let mut dot = false;
    let mut count = 0i64;
    let mut point = 0i64;
    let mut kept = 0i64;
    let mut mantissa = 0u64;
    let mut truncated = false;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'_' {
            if !last_digit {
                return None;
            }
            last_digit = false;
            pending_underscore = true;
        } else if byte == b'.' {
            if dot || pending_underscore {
                return None;
            }
            dot = true;
            point = count;
            last_digit = false;
        } else if let Some(digit) = (byte as char).to_digit(if hex { 16 } else { 10 }) {
            digits = true;
            last_digit = true;
            pending_underscore = false;
            if digit == 0 && count == 0 {
                point = point.saturating_sub(1);
            } else {
                count = count.saturating_add(1);
                if hex && kept < 16 {
                    mantissa = mantissa * 16 + u64::from(digit);
                    kept += 1;
                } else if hex && digit != 0 {
                    truncated = true;
                }
            }
        } else {
            break;
        }
        index += 1;
    }
    if !digits || pending_underscore {
        return None;
    }
    if !dot {
        point = count;
    }
    let mut exponent = 0i64;
    if bytes
        .get(index)
        .is_some_and(|b| b.eq_ignore_ascii_case(&if hex { b'p' } else { b'e' }))
    {
        index += 1;
        let exponent_negative = bytes.get(index) == Some(&b'-');
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let mut exponent_digits = false;
        let mut previous_digit = false;
        while let Some(&byte) = bytes.get(index) {
            if byte.is_ascii_digit() {
                exponent_digits = true;
                previous_digit = true;
                exponent = (exponent * 10 + i64::from(byte - b'0')).min(100_000);
            } else if byte == b'_' && previous_digit {
                previous_digit = false;
            } else {
                return None;
            }
            index += 1;
        }
        if !exponent_digits || !previous_digit {
            return None;
        }
        if exponent_negative {
            exponent = -exponent;
        }
    } else if hex {
        return None;
    }
    if index != bytes.len() {
        return None;
    }
    if !hex {
        let clean = value.replace('_', "");
        let parsed = clean.parse::<f64>().ok()?;
        return parsed.is_finite().then_some(parsed);
    }
    let mut exp = point
        .saturating_mul(4)
        .saturating_add(exponent)
        .saturating_sub(kept * 4)
        .saturating_add(52);
    while mantissa != 0 && mantissa >> 54 == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if truncated {
        mantissa |= 1;
    }
    while mantissa >> 55 != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    while mantissa > 1 && exp < -1024 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == (1 << 53) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> 52 == 0 {
        exp = -1023;
    }
    if exp > 1023 {
        return None;
    }
    let bits =
        (mantissa & ((1 << 52) - 1)) | (((exp + 1023) as u64) << 52) | (u64::from(negative) << 63);
    Some(f64::from_bits(bits))
}

pub(super) fn call(name: &str, arguments: &[Value]) -> Result<Value, EvaluationError> {
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
        ("values", [Value::Object(values)]) => Ok(Value::Array(values.values().cloned().collect())),
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
        ("to_array", [value]) => Ok(Value::Array(vec![value.clone()])),
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
