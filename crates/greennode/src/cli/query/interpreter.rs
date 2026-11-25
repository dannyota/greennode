// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use std::collections::BTreeMap;

use crate::cli::render::Value;

use super::{
    CompiledQuery,
    lexer::Kind,
    parser::{Node, NodeKind, NodeValue},
};

#[derive(Debug, PartialEq, Eq)]
pub(in crate::cli) enum EvaluationError {
    Unsupported,
    Depth,
    InvalidSlice,
    SliceOverflow,
    InvalidNode,
    NonFinite,
    FunctionArity,
    FunctionVariadicArity,
    FunctionType,
    UnknownFunction,
    Uncomparable,
    FunctionComparison,
}

pub(super) fn check(value: &Value, depth: usize) -> Result<(), EvaluationError> {
    if depth > 128 {
        return Err(EvaluationError::Depth);
    }
    match value {
        Value::Number(n) if !n.is_finite() => Err(EvaluationError::NonFinite),
        Value::Array(items) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            for item in items {
                check(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(items) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            for item in items.values() {
                check(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn child(node: &Node, index: usize) -> Result<&Node, EvaluationError> {
    node.children.get(index).ok_or(EvaluationError::InvalidNode)
}

pub(in crate::cli) fn evaluate(
    query: &CompiledQuery,
    input: &Value,
) -> Result<Value, EvaluationError> {
    check(input, 0)?;
    let result = execute(&query.0, input)?;
    check(&result, 0)?;
    Ok(result)
}

fn copy(value: &Value) -> Result<Value, EvaluationError> {
    check(value, 0)?;
    Ok(value.clone())
}

fn push(items: &mut Vec<Value>, value: Value) -> Result<(), EvaluationError> {
    check(&value, 1)?;
    items.push(value);
    Ok(())
}

fn source(node: &Node, input: &Value) -> Result<Value, EvaluationError> {
    match execute(node, input) {
        Err(EvaluationError::InvalidSlice) => Ok(Value::Null),
        other => other,
    }
}

fn is_false(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

pub(super) fn execute(node: &Node, input: &Value) -> Result<Value, EvaluationError> {
    match node.kind {
        NodeKind::FunctionExpression => {
            let NodeValue::Text(name) = &node.value else {
                return Err(EvaluationError::InvalidNode);
            };
            super::functions_expr::call(name, &node.children, input)
        }
        NodeKind::Comparator => {
            let left = execute(child(node, 0)?, input)?;
            let right = execute(child(node, 1)?, input)?;
            let NodeValue::Comparator(kind) = node.value else {
                return Err(EvaluationError::InvalidNode);
            };
            match kind {
                Kind::Eq => Ok(Value::Bool(left == right)),
                Kind::Ne => Ok(Value::Bool(left != right)),
                Kind::Lt | Kind::Lte | Kind::Gt | Kind::Gte => {
                    let (Value::Number(left), Value::Number(right)) = (left, right) else {
                        return Ok(Value::Null);
                    };
                    Ok(Value::Bool(match kind {
                        Kind::Lt => left < right,
                        Kind::Lte => left <= right,
                        Kind::Gt => left > right,
                        Kind::Gte => left >= right,
                        _ => return Err(EvaluationError::InvalidNode),
                    }))
                }
                _ => Err(EvaluationError::InvalidNode),
            }
        }
        NodeKind::OrExpression | NodeKind::AndExpression => {
            let left = execute(child(node, 0)?, input)?;
            if is_false(&left) == (node.kind == NodeKind::OrExpression) {
                execute(child(node, 1)?, input)
            } else {
                Ok(left)
            }
        }
        NodeKind::NotExpression => Ok(Value::Bool(is_false(&execute(child(node, 0)?, input)?))),
        NodeKind::Identity | NodeKind::CurrentNode => copy(input),
        NodeKind::Literal => match &node.value {
            NodeValue::Literal(value) => copy(value),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Field => match (&node.value, input) {
            (NodeValue::Text(key), Value::Object(map)) => match map.get(key) {
                Some(value) => copy(value),
                None => Ok(Value::Null),
            },
            (NodeValue::Text(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Subexpression | NodeKind::IndexExpression | NodeKind::Pipe => {
            let left = execute(child(node, 0)?, input)?;
            execute(child(node, 1)?, &left)
        }
        NodeKind::KeyValPair => execute(child(node, 0)?, input),
        NodeKind::Index => match (&node.value, input) {
            (NodeValue::Index(index), Value::Array(items)) => {
                let size = i128::try_from(items.len()).map_err(|_| EvaluationError::InvalidNode)?;
                let mut index = i128::from(*index);
                if index < 0 {
                    index += size;
                }
                match usize::try_from(index).ok().and_then(|n| items.get(n)) {
                    Some(value) => copy(value),
                    None => Ok(Value::Null),
                }
            }
            (NodeValue::Index(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Slice => match (&node.value, input) {
            (NodeValue::Slice(parts), Value::Array(items)) => slice(items, *parts),
            (NodeValue::Slice(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::FilterProjection => {
            let Value::Array(items) = source(child(node, 0)?, input)? else {
                return Ok(Value::Null);
            };
            let mut result = Vec::new();
            for item in &items {
                if !is_false(&execute(child(node, 2)?, item)?) {
                    let value = execute(child(node, 1)?, item)?;
                    if value != Value::Null {
                        push(&mut result, value)?;
                    }
                }
            }
            Ok(Value::Array(result))
        }
        NodeKind::Flatten => {
            let source = source(child(node, 0)?, input)?;
            let Value::Array(items) = source else {
                return Ok(Value::Null);
            };
            let mut result = Vec::new();
            for value in items {
                match value {
                    Value::Array(nested) => {
                        for item in nested {
                            push(&mut result, item)?;
                        }
                    }
                    item => push(&mut result, item)?,
                }
            }
            Ok(Value::Array(result))
        }
        NodeKind::Projection | NodeKind::ValueProjection => {
            let source = if node.kind == NodeKind::ValueProjection {
                source(child(node, 0)?, input)?
            } else {
                execute(child(node, 0)?, input)?
            };
            let mut result = Vec::new();
            match (node.kind, source) {
                (NodeKind::Projection, Value::Array(items)) => {
                    for item in &items {
                        let value = execute(child(node, 1)?, item)?;
                        if value != Value::Null {
                            push(&mut result, value)?;
                        }
                    }
                }
                (NodeKind::ValueProjection, Value::Object(items)) => {
                    for item in items.values() {
                        let value = execute(child(node, 1)?, item)?;
                        if value != Value::Null {
                            push(&mut result, value)?;
                        }
                    }
                }
                _ => return Ok(Value::Null),
            }
            Ok(Value::Array(result))
        }
        NodeKind::MultiSelectList => {
            if *input == Value::Null {
                return Ok(Value::Null);
            }
            let mut result = Vec::new();
            for child in &node.children {
                push(&mut result, execute(child, input)?)?;
            }
            Ok(Value::Array(result))
        }
        NodeKind::MultiSelectHash => {
            if *input == Value::Null {
                return Ok(Value::Null);
            }
            let mut result = BTreeMap::new();
            for child in &node.children {
                let NodeValue::Text(key) = &child.value else {
                    return Err(EvaluationError::InvalidNode);
                };
                let value = execute(child, input)?;
                check(&value, 1)?;
                result.insert(key.clone(), value);
            }
            Ok(Value::Object(result))
        }
        _ => Err(EvaluationError::Unsupported),
    }
}

fn slice(items: &[Value], parts: [Option<i64>; 3]) -> Result<Value, EvaluationError> {
    let length = i128::try_from(items.len()).map_err(|_| EvaluationError::InvalidSlice)?;
    let step = i128::from(parts[2].unwrap_or(1));
    if step == 0 {
        return Err(EvaluationError::InvalidSlice);
    }
    let cap = |n: i64| {
        let mut n = i128::from(n);
        if n < 0 {
            n += length;
        }
        if step < 0 {
            n.clamp(-1, length - 1)
        } else {
            n.clamp(0, length)
        }
    };
    let mut index = parts[0]
        .map(cap)
        .unwrap_or(if step < 0 { length - 1 } else { 0 });
    let stop = parts[1]
        .map(cap)
        .unwrap_or(if step < 0 { -1 } else { length });
    let mut result = Vec::new();
    while if step < 0 { index > stop } else { index < stop } {
        let position = usize::try_from(index).map_err(|_| EvaluationError::InvalidSlice)?;
        let value = items.get(position).ok_or(EvaluationError::InvalidSlice)?;
        check(value, 1)?;
        result.push(value.clone());
        let next = i64::try_from(index)
            .map_err(|_| EvaluationError::SliceOverflow)?
            .checked_add(i64::try_from(step).map_err(|_| EvaluationError::SliceOverflow)?)
            .ok_or(EvaluationError::SliceOverflow)?;
        index = i128::from(next);
    }
    Ok(Value::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_suppresses_source_error_but_propagates_predicate_error() {
        let numbers = Value::Array(vec![
            Value::Number(1.0),
            Value::Number(2.0),
            Value::Number(3.0),
        ]);
        let nested = Value::Array(vec![
            Value::Array(vec![Value::Number(0.0)]),
            Value::Array(vec![Value::Number(1.0)]),
        ]);
        assert_eq!(
            evaluate(&super::super::compile("(@[::0])[?@]").unwrap(), &numbers),
            Ok(Value::Null)
        );
        assert_eq!(
            evaluate(&super::super::compile("[?@[::0]]").unwrap(), &nested),
            Err(EvaluationError::InvalidSlice)
        );
    }
    #[test]
    fn filter_checks_result_depth_before_comparison() {
        let nested = |count| (0..count).fold(Value::Null, |value, _| Value::Array(vec![value]));
        let query = super::super::compile("([?`true`].[@]) == `[]`").unwrap();
        assert_eq!(evaluate(&query, &nested(127)), Ok(Value::Bool(false)));
        assert_eq!(evaluate(&query, &nested(128)), Err(EvaluationError::Depth));
    }
    #[test]
    fn basic_functions_match_go_results() {
        let run = |expression| evaluate(&super::super::compile(expression).unwrap(), &Value::Null);
        for (expression, expected) in [
            ("avg(`[1,2,3]`)", Value::Number(2.0)),
            ("ceil(`-1.2`)", Value::Number(-1.0)),
            ("floor(`-1.2`)", Value::Number(-2.0)),
            ("type(`false`)", Value::String("boolean".into())),
            ("not_null(`null`, `false`, `1`)", Value::Bool(false)),
            ("reverse('é🌿a')", Value::String("a🌿é".into())),
            (
                "reverse(`[1,2]`)",
                Value::Array(vec![Value::Number(2.0), Value::Number(1.0)]),
            ),
        ] {
            assert_eq!(run(expression), Ok(expected));
        }
        assert_eq!(run("avg(`[]`)"), Err(EvaluationError::NonFinite));
    }
    #[test]
    fn length_counts_unicode_code_points_and_checks_type() {
        let run = |expression| evaluate(&super::super::compile(expression).unwrap(), &Value::Null);
        assert_eq!(run("length('é🌿')"), Ok(Value::Number(2.0)));
        assert_eq!(run("length(`[1,2,3]`)"), Ok(Value::Number(3.0)));
        assert_eq!(run("length(`true`)"), Err(EvaluationError::FunctionType));
    }
    #[test]
    fn function_dispatch_checks_signatures() {
        let run = |expression| evaluate(&super::super::compile(expression).unwrap(), &Value::Null);
        for (expression, expected) in [
            ("ceil(`1`, `2`)", "FunctionArity"),
            ("ceil('1')", "FunctionType"),
            ("merge()", "FunctionVariadicArity"),
            ("merge(`false`)", "FunctionType"),
            ("not_null()", "FunctionVariadicArity"),
            ("unknown(`1`)", "UnknownFunction"),
        ] {
            assert_eq!(format!("{:?}", run(expression).unwrap_err()), expected);
        }
    }
    #[test]
    fn abs_checks_arity_and_numeric_type() {
        let eval = |expression| evaluate(&super::super::compile(expression).unwrap(), &Value::Null);
        assert_eq!(eval("abs(`-2`)"), Ok(Value::Number(2.0)));
        assert_eq!(eval("abs('2')"), Err(EvaluationError::FunctionType));
        assert_eq!(eval("abs(`1`, `2`)"), Err(EvaluationError::FunctionArity));
    }
    #[test]
    fn filter_drops_null_results_and_false_predicates() {
        let input = crate::cli::render::project(&serde_json::json!([
            {"keep": true, "name": "yes"}, {"keep": true}, {"keep": false, "name": "no"}
        ]))
        .unwrap()
        .for_query()
        .unwrap();
        assert_eq!(
            evaluate(&super::super::compile("[?keep].name").unwrap(), &input),
            Ok(Value::Array(vec![Value::String("yes".into())]))
        );
        assert_eq!(
            evaluate(
                &super::super::compile("[?keep].name").unwrap(),
                &Value::Null
            ),
            Ok(Value::Null)
        );
    }
    #[test]
    fn comparator_uses_exact_numbers_and_numeric_ordering() {
        for (expression, expected) in [
            ("`1` == `1.0`", Value::Bool(true)),
            ("`1` == `1.000000000000001`", Value::Bool(false)),
            (
                "`9007199254740992` == `9007199254740993`",
                Value::Bool(true),
            ),
            ("`[1,{\"a\":2}]` == `[1.0,{\"a\":2.0}]`", Value::Bool(true)),
            ("'a' < 'b'", Value::Null),
            ("`1` < '2'", Value::Null),
            ("`1` < `2`", Value::Bool(true)),
        ] {
            assert_eq!(
                evaluate(&super::super::compile(expression).unwrap(), &Value::Null),
                Ok(expected)
            );
        }
    }
    #[test]
    fn boolean_short_circuit_preserves_values() {
        for (expression, expected) in [
            ("`false` && unknown(@)", Value::Bool(false)),
            ("`0` || unknown(@)", Value::Number(0.0)),
            ("`[]` || `7`", Value::Number(7.0)),
            ("!`{}`", Value::Bool(true)),
            ("!`0`", Value::Bool(false)),
        ] {
            assert_eq!(
                evaluate(&super::super::compile(expression).unwrap(), &Value::Null),
                Ok(expected)
            );
        }
    }
    #[test]
    fn current_node_returns_input() {
        let query = super::super::compile("@").unwrap();
        assert_eq!(evaluate(&query, &Value::Bool(true)), Ok(Value::Bool(true)));
    }
    fn supported(node: &Node) -> bool {
        !matches!(
            node.kind,
            NodeKind::ExpRef | NodeKind::FunctionExpression | NodeKind::Empty
        ) && node.children.iter().all(supported)
    }

    #[test]
    fn functions_compliance() {
        let path = format!(
            "{}/tests/fixtures/query/compliance/functions.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut passed = 0;
        let mut mismatches = Vec::new();
        for (group_index, group) in data.as_array().unwrap().iter().enumerate() {
            let input = crate::cli::render::project(&group["given"])
                .unwrap()
                .for_query()
                .unwrap();
            for (case_index, case) in group["cases"].as_array().unwrap().iter().enumerate() {
                let id = format!("{group_index}:{case_index}");
                let expression = case["expression"].as_str().unwrap();
                let query = match super::super::compile(expression) {
                    Ok(query) => query,
                    Err(_) if case["error"].is_string() => {
                        passed += 1;
                        continue;
                    }
                    Err(_) => {
                        mismatches.push(format!("{id}: compile {expression}"));
                        continue;
                    }
                };
                let actual = evaluate(&query, &input);
                let matched = if case["error"].is_string() {
                    actual.is_err()
                } else {
                    let expected = crate::cli::render::project(&case["result"])
                        .unwrap()
                        .for_query()
                        .unwrap();
                    actual.as_ref().is_ok_and(|value| *value == expected)
                };
                if matched {
                    passed += 1;
                } else {
                    mismatches.push(format!(
                        "{id}: {expression}: {actual:?} expected {}",
                        case["result"]
                    ));
                }
            }
        }
        eprintln!(
            "functions: {passed} passed, {} mismatches",
            mismatches.len()
        );
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
        assert_eq!(passed, 171);
    }

    #[test]
    fn selected_compliance() {
        let files = [
            "basic",
            "boolean",
            "filters",
            "current",
            "escape",
            "identifiers",
            "indices",
            "literal",
            "multiselect",
            "pipe",
            "slice",
            "unicode",
            "wildcard",
        ];
        let mut passed = 0;
        let mut skipped = Vec::new();
        let mut syntax_rejections = 0;
        let mut mismatches = Vec::new();
        for file in files {
            let path = format!(
                "{}/tests/fixtures/query/compliance/{file}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let data = std::fs::read_to_string(path).unwrap();
            let suites: serde_json::Value = serde_json::from_str(&data).unwrap();
            for (suite_index, suite) in suites.as_array().unwrap().iter().enumerate() {
                let input = crate::cli::render::project(&suite["given"])
                    .unwrap()
                    .for_query()
                    .unwrap();
                for (case_index, case) in suite["cases"].as_array().unwrap().iter().enumerate() {
                    let expression = case["expression"].as_str().unwrap();
                    let expected_error = case["error"].is_string();
                    let query = match super::super::compile(expression) {
                        Ok(query) => query,
                        Err(_) if expected_error => {
                            syntax_rejections += 1;
                            continue;
                        }
                        Err(_) => {
                            mismatches.push(format!("{file}: {expression:?} failed to compile"));
                            continue;
                        }
                    };
                    if !supported(&query.0) {
                        skipped.push(format!("{file}:{suite_index}:{case_index}:{expression}"));
                        continue;
                    }
                    let actual = evaluate(&query, &input);
                    let matched = if expected_error {
                        actual.is_err()
                    } else {
                        let expected = crate::cli::render::project(&case["result"])
                            .unwrap()
                            .for_query()
                            .unwrap();
                        actual.as_ref().is_ok_and(|value| *value == expected)
                    };
                    if matched {
                        passed += 1;
                    } else {
                        mismatches.push(format!(
                            "{file}: {expression:?}: {actual:?} expected {}",
                            case["result"]
                        ));
                    }
                }
            }
        }
        eprintln!(
            "compliance: {passed} evaluated, {syntax_rejections} syntax errors, {} unsupported, {} mismatches",
            skipped.len(),
            mismatches.len()
        );
        assert_eq!(passed, 552);
        assert_eq!(syntax_rejections, 4);
        assert!(skipped.is_empty(), "{}", skipped.join("\n"));
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
    #[test]
    fn go_slice_index_overflow_is_error() {
        let input = Value::Array(vec![
            Value::Number(11.0),
            Value::Number(22.0),
            Value::Number(33.0),
        ]);
        let bad = super::super::compile("@[1::9223372036854775807]").unwrap();
        let good = super::super::compile("@[1::2]").unwrap();
        assert!(evaluate(&bad, &input).is_err());
        assert_eq!(
            evaluate(&good, &input),
            Ok(Value::Array(vec![Value::Number(22.0)]))
        );
    }

    #[test]
    fn source_slice_errors_are_suppressed_only_on_go_nodes() {
        let input = Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]);
        for expression in ["@[::0][]", "(@[::0]).*"] {
            assert_eq!(
                evaluate(&super::super::compile(expression).unwrap(), &input),
                Ok(Value::Null)
            );
        }
        assert_eq!(
            evaluate(&super::super::compile("@[::0]").unwrap(), &input),
            Err(EvaluationError::InvalidSlice)
        );
        for expression in [
            "(@[1::9223372036854775807])[]",
            "(@[1::9223372036854775807]).*",
        ] {
            assert_eq!(
                evaluate(&super::super::compile(expression).unwrap(), &input),
                Err(EvaluationError::SliceOverflow)
            );
        }
        assert_eq!(
            evaluate(&super::super::compile("(foo(@)).*").unwrap(), &input),
            Err(EvaluationError::UnknownFunction)
        );
    }

    #[test]
    fn empty_container_depth_matches_renderers() {
        let nested =
            |count| (0..count).fold(Value::Array(vec![]), |value, _| Value::Array(vec![value]));
        let query = super::super::compile("@").unwrap();
        let valid = evaluate(&query, &nested(127)).unwrap();
        assert!(crate::cli::render::write_text(&mut Vec::new(), &valid).is_ok());
        assert!(crate::cli::render::write_table(&mut Vec::new(), &valid).is_ok());
        assert!(crate::cli::render::project(&valid).is_ok());
        assert_eq!(evaluate(&query, &nested(128)), Err(EvaluationError::Depth));
        assert!(crate::cli::render::write_text(&mut Vec::new(), &nested(128)).is_err());
        assert!(crate::cli::render::write_table(&mut Vec::new(), &nested(128)).is_err());
        assert!(crate::cli::render::project(&nested(128)).is_err());
    }

    #[test]
    fn wildcard_order_uses_utf8_key_bytes() {
        let input = Value::Object(BTreeMap::from([
            ("é".into(), Value::Number(3.0)),
            ("z".into(), Value::Number(2.0)),
            ("A".into(), Value::Number(1.0)),
        ]));
        let expected = Value::Array(vec![
            Value::Number(1.0),
            Value::Number(2.0),
            Value::Number(3.0),
        ]);
        assert_eq!(
            evaluate(&super::super::compile("*").unwrap(), &input),
            Ok(expected)
        );
    }

    #[test]
    fn depth_limit() {
        let nested = |count| (0..count).fold(Value::Null, |value, _| Value::Array(vec![value]));
        let query = super::super::compile("@").unwrap();
        assert!(evaluate(&query, &nested(128)).is_ok());
        assert_eq!(evaluate(&query, &nested(129)), Err(EvaluationError::Depth));
        let list = super::super::compile("[@]").unwrap();
        assert!(evaluate(&list, &nested(127)).is_ok());
        assert_eq!(evaluate(&list, &nested(128)), Err(EvaluationError::Depth));
    }
}
