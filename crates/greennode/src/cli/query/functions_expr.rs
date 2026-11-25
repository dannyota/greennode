// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use super::{
    interpreter::{EvaluationError, WorkBudget, check, execute},
    parser::{Node, NodeKind},
};
use crate::cli::render::Value;

enum Argument<'a> {
    Value(Value),
    Reference(&'a Node),
}

pub(super) fn call(
    name: &str,
    nodes: &[Node],
    input: &Value,
    budget: &mut WorkBudget,
) -> Result<Value, EvaluationError> {
    let mut arguments = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.kind == NodeKind::ExpRef {
            budget.charge(1)?;
            arguments.push(Argument::Reference(
                node.children.first().ok_or(EvaluationError::InvalidNode)?,
            ));
        } else {
            arguments.push(Argument::Value(execute(node, input, budget)?));
        }
    }
    if !matches!(name, "map" | "sort_by" | "max_by" | "min_by") {
        return call_regular(name, &arguments, budget);
    }
    let [first, second] = arguments.as_slice() else {
        return Err(EvaluationError::FunctionArity);
    };
    let (reference, values) = if name == "map" {
        match (first, second) {
            (Argument::Reference(reference), Argument::Value(Value::Array(values))) => {
                (*reference, values)
            }
            _ => return Err(EvaluationError::FunctionType),
        }
    } else {
        match (first, second) {
            (Argument::Value(Value::Array(values)), Argument::Reference(reference)) => {
                (*reference, values)
            }
            _ => return Err(EvaluationError::FunctionType),
        }
    };
    if name == "map" {
        let mut result = Vec::with_capacity(values.len());
        let mut ordinary = None;
        for value in values {
            match execute(reference, value, budget).and_then(|item| {
                check(&item, 1)?;
                Ok(item)
            }) {
                Ok(item) => result.push(item),
                Err(error) => record_error(error, &mut ordinary)?,
            }
        }
        if let Some(error) = ordinary {
            return Err(error);
        }
        return Ok(Value::Array(result));
    }
    if values.len() < 2 {
        return if name == "sort_by" {
            Ok(Value::Array(values.clone()))
        } else {
            Ok(values.first().cloned().unwrap_or(Value::Null))
        };
    }
    let mut ordinary = None;
    let mut numeric = None;
    let mut keys = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        match execute(reference, value, budget) {
            Ok(key) => {
                if index == 0 {
                    numeric = match key {
                        Value::Number(_) => Some(true),
                        Value::String(_) => Some(false),
                        _ => {
                            record_error(EvaluationError::FunctionType, &mut ordinary)?;
                            None
                        }
                    };
                } else if let Some(numeric) = numeric
                    && !matches!(
                        (&key, numeric),
                        (Value::Number(_), true) | (Value::String(_), false)
                    )
                {
                    let error = if name == "sort_by" {
                        EvaluationError::FunctionComparison
                    } else {
                        EvaluationError::FunctionType
                    };
                    record_error(error, &mut ordinary)?;
                }
                keys.push(key);
            }
            Err(error) => {
                let error = if name == "sort_by" && index > 0 {
                    comparison_error(error)
                } else {
                    error
                };
                record_error(error, &mut ordinary)?;
            }
        }
    }
    if let Some(error) = ordinary {
        return Err(error);
    }
    let numeric = numeric.ok_or(EvaluationError::InvalidNode)?;
    if name != "sort_by" {
        let mut best = 0;
        for index in 1..keys.len() {
            let ordering = compare_keys(&keys[index], &keys[best], numeric)
                .ok_or(EvaluationError::FunctionType)?;
            if (name == "max_by" && ordering.is_gt()) || (name == "min_by" && ordering.is_lt()) {
                best = index;
            }
        }
        return Ok(values[best].clone());
    }
    if numeric {
        let mut indexed = keys
            .into_iter()
            .zip(values.iter())
            .map(|(key, value)| match key {
                Value::Number(number) if number.is_finite() => Ok((number, value)),
                Value::Number(_) => Err(EvaluationError::NonFinite),
                _ => Err(EvaluationError::FunctionComparison),
            })
            .collect::<Result<Vec<_>, _>>()?;
        indexed.sort_by(|(a, _), (b, _)| {
            if a == b {
                std::cmp::Ordering::Equal
            } else {
                a.total_cmp(b)
            }
        });
        Ok(Value::Array(
            indexed
                .into_iter()
                .map(|(_, value)| value.clone())
                .collect(),
        ))
    } else {
        let mut indexed = keys
            .into_iter()
            .zip(values.iter())
            .map(|(key, value)| match key {
                Value::String(string) => Ok((string, value)),
                _ => Err(EvaluationError::FunctionComparison),
            })
            .collect::<Result<Vec<_>, _>>()?;
        indexed.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(Value::Array(
            indexed
                .into_iter()
                .map(|(_, value)| value.clone())
                .collect(),
        ))
    }
}

fn record_error(
    error: EvaluationError,
    ordinary: &mut Option<EvaluationError>,
) -> Result<(), EvaluationError> {
    if error.is_safety() {
        return Err(error);
    }
    if ordinary.is_none() {
        *ordinary = Some(error);
    }
    Ok(())
}

fn comparison_error(error: EvaluationError) -> EvaluationError {
    if error.is_safety() {
        error
    } else {
        EvaluationError::FunctionComparison
    }
}

fn call_regular(
    name: &str,
    arguments: &[Argument<'_>],
    budget: &mut WorkBudget,
) -> Result<Value, EvaluationError> {
    use super::functions::{self, Type};
    let signature = functions::signature(name).ok_or(EvaluationError::UnknownFunction)?;
    if signature.variadic {
        if arguments.len() < signature.arguments.len() {
            return Err(EvaluationError::FunctionVariadicArity);
        }
    } else {
        if arguments.len() != signature.arguments.len() {
            return Err(EvaluationError::FunctionArity);
        }
        for (arg, allowed) in arguments.iter().zip(signature.arguments) {
            let valid = allowed.iter().any(|kind| match arg {
                Argument::Value(value) => functions::matches_type(value, *kind),
                Argument::Reference(_) => matches!(kind, Type::Any | Type::Expref),
            });
            if !valid {
                return Err(EvaluationError::FunctionType);
            }
        }
    }
    if arguments
        .iter()
        .all(|arg| matches!(arg, Argument::Value(_)))
    {
        let values = arguments
            .iter()
            .filter_map(|arg| match arg {
                Argument::Value(value) => Some(value.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        return functions::call_budgeted(name, &values, budget);
    }
    match (name, arguments) {
        (
            "contains",
            [
                Argument::Value(Value::String(_) | Value::Array(_)),
                Argument::Reference(_),
            ],
        ) => Ok(Value::Bool(false)),
        ("to_string", [Argument::Reference(_)]) => Ok(Value::String("{}".into())),
        ("not_null", values) => {
            for value in values {
                match value {
                    Argument::Value(Value::Null) => {}
                    Argument::Value(value) => return Ok(value.clone()),
                    Argument::Reference(_) => return Err(EvaluationError::Unsupported),
                }
            }
            Ok(Value::Null)
        }
        ("merge", _) => Err(EvaluationError::FunctionPanic),
        ("to_array", [Argument::Reference(_)]) => Err(EvaluationError::Unsupported),
        _ => Err(EvaluationError::FunctionType),
    }
}

fn compare_keys(a: &Value, b: &Value, numeric: bool) -> Option<std::cmp::Ordering> {
    match (a, b, numeric) {
        (Value::Number(a), Value::Number(b), true) => a.partial_cmp(b),
        (Value::String(a), Value::String(b), false) => Some(a.cmp(b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{compile, evaluate};
    use crate::cli::render::Value;

    #[test]
    fn direct_references_reach_any_function() {
        let input = Value::Bool(true);
        for (expression, expected) in [
            ("contains('abc', &foo)", Value::Bool(false)),
            ("to_string(&foo)", Value::String("{}".into())),
            ("not_null(`1`, &foo)", Value::Number(1.0)),
        ] {
            assert_eq!(
                evaluate(&compile(expression).unwrap(), &input),
                Ok(expected)
            );
        }
    }

    #[test]
    fn sort_by_preserves_input_array() {
        let input = Value::Array(vec![Value::Number(2.0), Value::Number(1.0)]);
        assert_eq!(
            evaluate(&compile("[sort_by(@,&@),@[0]]").unwrap(), &input),
            Ok(Value::Array(vec![
                Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]),
                Value::Number(2.0)
            ]))
        );
    }

    #[test]
    fn sort_by_keeps_signed_zero_order() {
        let query = compile("sort_by(@, &@)").unwrap();
        let input = Value::Array(vec![Value::Number(0.0), Value::Number(-0.0)]);
        let Value::Array(values) = evaluate(&query, &input).unwrap() else {
            panic!("expected array");
        };
        let [Value::Number(first), Value::Number(second)] = values.as_slice() else {
            panic!("expected numbers");
        };
        assert_eq!(
            (first.to_bits(), second.to_bits()),
            (0.0f64.to_bits(), (-0.0f64).to_bits())
        );
    }

    #[test]
    fn later_safety_errors_are_not_hidden_by_ordinary_errors() {
        let input = crate::cli::render::project(&serde_json::json!([{"k":1},{"a":true},{}]))
            .unwrap()
            .for_query()
            .unwrap();
        for expression in [
            "(sort_by(@, &(k || (a && unknown()) || merge(`null`)))).*",
            "(max_by(@, &(k || (a && unknown()) || merge(`null`)))).*",
            "(min_by(@, &(k || (a && unknown()) || merge(`null`)))).*",
            "(map(&(k || (a && unknown()) || merge(`null`)), @)).*",
        ] {
            assert_eq!(
                evaluate(&compile(expression).unwrap(), &input),
                Err(super::EvaluationError::FunctionPanic),
                "{expression}"
            );
        }
    }

    #[test]
    fn first_ordinary_error_is_retained() {
        let input = crate::cli::render::project(&serde_json::json!([{"a":true},{}]))
            .unwrap()
            .for_query()
            .unwrap();
        for expression in [
            "sort_by(@, &(a && unknown() || length(`null`)))",
            "max_by(@, &(a && unknown() || length(`null`)))",
            "min_by(@, &(a && unknown() || length(`null`)))",
            "map(&(a && unknown() || length(`null`)), @)",
        ] {
            assert_eq!(
                evaluate(&compile(expression).unwrap(), &input),
                Err(super::EvaluationError::UnknownFunction),
                "{expression}"
            );
        }
    }

    #[test]
    fn sort_by_rejects_mixed_keys() {
        let query = compile("sort_by(`[1,null,2]`, &@)").unwrap();
        assert_eq!(
            evaluate(&query, &Value::Null),
            Err(super::EvaluationError::FunctionComparison)
        );
    }

    #[test]
    fn sort_by_stays_stable_across_merge_blocks() {
        use std::collections::BTreeMap;
        let input = Value::Array(
            (0..61)
                .map(|i| {
                    Value::Object(BTreeMap::from([
                        ("k".into(), Value::Number((i % 4) as f64)),
                        ("v".into(), Value::Number(i as f64)),
                    ]))
                })
                .collect(),
        );
        let expected = Value::Array(
            (0..4)
                .flat_map(|k| (0..61).filter(move |i| i % 4 == k))
                .map(|i| Value::Number(i as f64))
                .collect(),
        );
        assert_eq!(
            evaluate(&compile("sort_by(@, &k)[].v").unwrap(), &input),
            Ok(expected)
        );
    }

    #[test]
    fn expression_reference_functions() {
        let input = Value::Bool(true);
        for (expression, expected) in [
            (
                "map(&x, `[{\"x\":1},{\"x\":2}]`)",
                Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]),
            ),
            (
                "sort_by(`[ {\"k\":1,\"v\":\"a\"}, {\"k\":1,\"v\":\"b\"} ]`, &k)[].v",
                Value::Array(vec![Value::String("a".into()), Value::String("b".into())]),
            ),
            ("max_by(`[1,2]`, &@)", Value::Number(2.0)),
            ("min_by(`[1,2]`, &@)", Value::Number(1.0)),
        ] {
            assert_eq!(
                evaluate(&compile(expression).unwrap(), &input),
                Ok(expected)
            );
        }
        assert!(evaluate(&compile("sort_by(`[1,\"a\"]`, &@)").unwrap(), &input).is_err());
    }

    #[test]
    fn expression_references_outside_direct_arguments_fail() {
        let input = Value::Bool(true);
        for expression in [
            "&foo",
            "[&foo]",
            "{a: &foo}",
            "[&foo][0]",
            "&foo | @",
            "[&foo][*]",
            "not_null(&foo)",
        ] {
            assert!(
                evaluate(&compile(expression).unwrap(), &input).is_err(),
                "{expression}"
            );
        }
    }
}
