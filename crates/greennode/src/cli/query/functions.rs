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
