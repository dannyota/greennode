// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use std::collections::BTreeMap;

use crate::cli::render::Value;

use super::{
    CompiledQuery,
    parser::{Node, NodeKind, NodeValue},
};

#[derive(Debug, PartialEq, Eq)]
pub(in crate::cli) enum EvaluationError {
    Unsupported,
    Depth,
    InvalidSlice,
    InvalidNode,
    NonFinite,
}

fn check(value: &Value, depth: usize) -> Result<(), EvaluationError> {
    if depth > 128 {
        return Err(EvaluationError::Depth);
    }
    match value {
        Value::Number(n) if !n.is_finite() => Err(EvaluationError::NonFinite),
        Value::Array(items) => {
            for item in items {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
                check(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(items) => {
            for item in items.values() {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
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

fn execute(node: &Node, input: &Value) -> Result<Value, EvaluationError> {
    match node.kind {
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
        NodeKind::Flatten => {
            let source = execute(child(node, 0)?, input)?;
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
            let source = execute(child(node, 0)?, input)?;
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
        index = index
            .checked_add(step)
            .ok_or(EvaluationError::InvalidSlice)?;
    }
    Ok(Value::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_node_returns_input() {
        let query = super::super::compile("@").unwrap();
        assert_eq!(evaluate(&query, &Value::Bool(true)), Ok(Value::Bool(true)));
    }
    fn supported(node: &Node) -> bool {
        !matches!(
            node.kind,
            NodeKind::Comparator
                | NodeKind::ExpRef
                | NodeKind::FunctionExpression
                | NodeKind::FilterProjection
                | NodeKind::OrExpression
                | NodeKind::AndExpression
                | NodeKind::NotExpression
                | NodeKind::Empty
        ) && node.children.iter().all(supported)
    }
    #[test]
    fn selected_compliance() {
        let files = [
            "basic",
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
        let mut skipped = 0;
        let mut syntax_rejections = 0;
        let mut mismatches = Vec::new();
        for file in files {
            let path = format!(
                "{}/tests/fixtures/query/compliance/{file}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let data = std::fs::read_to_string(path).unwrap();
            let suites: serde_json::Value = serde_json::from_str(&data).unwrap();
            for suite in suites.as_array().unwrap() {
                let input = crate::cli::render::project(&suite["given"])
                    .unwrap()
                    .for_query()
                    .unwrap();
                for case in suite["cases"].as_array().unwrap() {
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
                        skipped += 1;
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
            "compliance: {passed} evaluated, {syntax_rejections} syntax errors, {skipped} unsupported, {} mismatches",
            mismatches.len()
        );
        assert_eq!(passed + syntax_rejections + skipped, 430);
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
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
