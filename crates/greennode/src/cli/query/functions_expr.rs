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
    let mut arguments = Vec::new();
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
        budget.charge(1)?;
        let mut result = Vec::new();
        let mut ordinary = None;
        for value in values {
            budget.charge(1)?;
            match execute(reference, value, budget).and_then(|item| {
                check(&item, 1, budget)?;
                Ok(item)
            }) {
                Ok(item) => {
                    budget.charge(1)?;
                    result.push(item);
                }
                Err(error) => record_error(error, &mut ordinary)?,
            }
        }
        if let Some(error) = ordinary {
            return Err(error);
        }
        return Ok(Value::Array(result));
    }
    if values.len() < 2 {
        if let Some(value) = values.first() {
            budget.charge_value(value)?;
        }
        return if name == "sort_by" {
            budget.charge(1)?;
            Ok(Value::Array(values.clone()))
        } else {
            Ok(values.first().cloned().unwrap_or(Value::Null))
        };
    }
    let mut ordinary = None;
    let mut numeric = None;
    let mut keys = Vec::new();
    for (index, value) in values.iter().enumerate() {
        budget.charge(1)?;
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
            budget.charge(2)?;
            if let (Value::String(left), Value::String(right)) = (&keys[index], &keys[best]) {
                budget.charge_compare_bytes(left, right)?;
            }
            let ordering = compare_keys(&keys[index], &keys[best], numeric)
                .ok_or(EvaluationError::FunctionType)?;
            if (name == "max_by" && ordering.is_gt()) || (name == "min_by" && ordering.is_lt()) {
                best = index;
            }
        }
        budget.charge_value(&values[best])?;
        return Ok(values[best].clone());
    }
    budget.charge(u64::try_from(keys.len()).map_err(|_| EvaluationError::WorkBudget)?)?;
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
        let meter = super::interpreter::SortMeter::new(budget);
        indexed.sort_by(|(a, _), (b, _)| meter.numbers(a, b));
        meter.finish(budget)?;
        budget.charge(1)?;
        let mut output = Vec::new();
        for (_, value) in indexed {
            budget.charge_value(value)?;
            output.push(value.clone());
        }
        Ok(Value::Array(output))
    } else {
        let mut indexed = keys
            .into_iter()
            .zip(values.iter())
            .map(|(key, value)| match key {
                Value::String(string) => Ok((string, value)),
                _ => Err(EvaluationError::FunctionComparison),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let meter = super::interpreter::SortMeter::new(budget);
        indexed.sort_by(|(a, _), (b, _)| meter.strings(a, b));
        meter.finish(budget)?;
        budget.charge(1)?;
        let mut output = Vec::new();
        for (_, value) in indexed {
            budget.charge_value(value)?;
            output.push(value.clone());
        }
        Ok(Value::Array(output))
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
            let mut valid = false;
            for kind in *allowed {
                let matches = match arg {
                    Argument::Value(value) => functions::matches_type(value, *kind, budget)?,
                    Argument::Reference(_) => matches!(kind, Type::Any | Type::Expref),
                };
                if matches {
                    valid = true;
                    break;
                }
            }
            if !valid {
                return Err(EvaluationError::FunctionType);
            }
        }
    }
    if arguments
        .iter()
        .all(|arg| matches!(arg, Argument::Value(_)))
    {
        let mut values = Vec::new();
        for argument in arguments {
            if let Argument::Value(value) = argument {
                budget.charge_value(value)?;
                values.push(value.clone());
            }
        }
        return functions::call_budgeted(name, &values, budget);
    }
    match (name, arguments) {
        ("contains", [Argument::Value(Value::String(_)), Argument::Reference(_)]) => {
            Ok(Value::Bool(false))
        }
        (
            "contains",
            [
                Argument::Value(Value::Array(values)),
                Argument::Reference(_),
            ],
        ) => {
            budget.charge(u64::try_from(values.len()).map_err(|_| EvaluationError::WorkBudget)?)?;
            Ok(Value::Bool(false))
        }
        ("to_string", [Argument::Reference(_)]) => {
            budget.charge(2)?;
            Ok(Value::String("{}".into()))
        }
        ("not_null", values) => {
            for value in values {
                budget.charge(1)?;
                match value {
                    Argument::Value(Value::Null) => {}
                    Argument::Value(value) => {
                        budget.charge_value(value)?;
                        return Ok(value.clone());
                    }
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
    fn sorted_by_strings_stay_within_budget() {
        let prefix = "x".repeat(256);
        let input = Value::Array(
            (0..999)
                .map(|n| Value::String(format!("{prefix}{n:04}")))
                .collect(),
        );
        assert!(
            matches!(evaluate(&compile("sort_by(@, &@)").unwrap(), &input), Ok(Value::Array(values)) if values.len() == 999)
        );
    }

    #[test]
    fn by_functions_charge_string_key_comparisons() {
        let prefix = "x".repeat(1_024);
        let input = Value::Array(vec![
            Value::String(format!("{prefix}a")),
            Value::String(format!("{prefix}b")),
        ]);
        for (name, used) in [
            ("max_by", 9_994_500),
            ("min_by", 9_994_500),
            ("sort_by", 9_993_300),
        ] {
            let query = compile(&format!("{name}(@, &@)")).unwrap();
            let mut budget = super::WorkBudget::new();
            budget.charge(used).unwrap();
            assert_eq!(
                super::call(name, &query.0.children, &input, &mut budget),
                Err(super::EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn contains_reference_charges_array_scan() {
        let query = compile("contains(@, &foo)").unwrap();
        let input = Value::Array(vec![Value::Number(1.0); 1_000]);
        let mut budget = super::WorkBudget::new();
        budget.charge(9_998_500).unwrap();
        assert_eq!(
            super::call("contains", &query.0.children, &input, &mut budget),
            Err(super::EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn extremum_by_charges_key_comparisons() {
        let input = Value::Array((0..1_000).rev().map(|n| Value::Number(n as f64)).collect());
        for name in ["max_by", "min_by"] {
            let query = compile(&format!("{name}(@, &@)")).unwrap();
            let mut budget = super::WorkBudget::new();
            budget.charge(9_995_000).unwrap();
            assert_eq!(
                super::call(name, &query.0.children, &input, &mut budget),
                Err(super::EvaluationError::WorkBudget),
                "{name}"
            );
        }
    }

    #[test]
    fn sort_by_charges_keys_comparisons_and_output() {
        let query = compile("sort_by(@, &@)").unwrap();
        let input = Value::Array((0..1_000).rev().map(|n| Value::Number(n as f64)).collect());
        let mut budget = super::WorkBudget::new();
        budget.charge(9_991_500).unwrap();
        assert_eq!(
            super::call("sort_by", &query.0.children, &input, &mut budget),
            Err(super::EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn map_charges_examined_and_produced_elements() {
        let query = compile("map(&@, @)").unwrap();
        let input = Value::Array(vec![Value::Number(1.0); 1_000]);
        let mut budget = super::WorkBudget::new();
        budget.charge(9_996_800).unwrap();
        assert_eq!(
            super::call("map", &query.0.children, &input, &mut budget),
            Err(super::EvaluationError::WorkBudget)
        );
    }

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
