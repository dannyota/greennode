// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use std::{cell::Cell, cmp::Ordering, collections::BTreeMap};

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
    FunctionPanic,
    WorkBudget,
}

impl EvaluationError {
    pub(super) fn is_safety(&self) -> bool {
        match self {
            Self::Unsupported
            | Self::Depth
            | Self::SliceOverflow
            | Self::InvalidNode
            | Self::NonFinite
            | Self::Uncomparable
            | Self::FunctionPanic
            | Self::WorkBudget => true,
            Self::InvalidSlice
            | Self::FunctionArity
            | Self::FunctionVariadicArity
            | Self::FunctionType
            | Self::UnknownFunction
            | Self::FunctionComparison => false,
        }
    }
}

pub(super) struct WorkBudget {
    used: u64,
}

impl WorkBudget {
    pub(super) fn new() -> Self {
        Self { used: 0 }
    }

    pub(super) fn equal(&mut self, left: &Value, right: &Value) -> Result<bool, EvaluationError> {
        self.equal_at(left, right, 0)
    }

    fn equal_at(
        &mut self,
        left: &Value,
        right: &Value,
        depth: usize,
    ) -> Result<bool, EvaluationError> {
        if depth > 128 {
            return Err(EvaluationError::Depth);
        }
        self.charge(1)?;
        match (left, right) {
            (Value::Array(a), Value::Array(b)) => {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (left, right) in a.iter().zip(b) {
                    if !self.equal_at(left, right, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Object(a), Value::Object(b)) => {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
                if a.len() != b.len() {
                    return Ok(false);
                }
                for ((ka, va), (kb, vb)) in a.iter().zip(b) {
                    self.charge_compare_bytes(ka, kb)?;
                    if ka != kb || !self.equal_at(va, vb, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::String(left), Value::String(right)) => {
                self.charge_compare_bytes(left, right)?;
                Ok(left == right)
            }
            _ => Ok(left == right),
        }
    }

    pub(super) fn charge_value(&mut self, value: &Value) -> Result<(), EvaluationError> {
        self.charge_value_at(value, 0)
    }

    fn charge_value_at(&mut self, value: &Value, depth: usize) -> Result<(), EvaluationError> {
        if depth > 128 {
            return Err(EvaluationError::Depth);
        }
        self.charge(1)?;
        match value {
            Value::Array(values) => {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
                for value in values {
                    self.charge_value_at(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                if depth >= 128 {
                    return Err(EvaluationError::Depth);
                }
                for (key, value) in values {
                    self.charge_bytes(key)?;
                    self.charge_value_at(value, depth + 1)?;
                }
            }
            Value::String(value) => self.charge_bytes(value)?,
            _ => {}
        }
        Ok(())
    }

    pub(super) fn charge_compare_bytes(
        &mut self,
        left: &str,
        right: &str,
    ) -> Result<(), EvaluationError> {
        self.charge(
            u64::try_from(left.len().min(right.len())).map_err(|_| EvaluationError::WorkBudget)?,
        )
    }

    pub(super) fn charge_map_key(
        &mut self,
        key: &str,
        entries: usize,
    ) -> Result<(), EvaluationError> {
        let bytes = u64::try_from(key.len()).map_err(|_| EvaluationError::WorkBudget)?;
        let count = u64::try_from(entries).map_err(|_| EvaluationError::WorkBudget)?;
        self.charge(
            bytes
                .checked_mul(count)
                .ok_or(EvaluationError::WorkBudget)?,
        )
    }

    pub(super) fn charge_merge_key(
        &mut self,
        key: &str,
        entries: usize,
    ) -> Result<(), EvaluationError> {
        let size = entries.checked_add(1).ok_or(EvaluationError::WorkBudget)?;
        let log = if size <= 1 {
            0
        } else {
            usize::BITS - (size - 1).leading_zeros()
        };
        let bytes = u64::try_from(key.len()).map_err(|_| EvaluationError::WorkBudget)?;
        self.charge(
            bytes
                .checked_mul(u64::from(log))
                .ok_or(EvaluationError::WorkBudget)?,
        )
    }

    pub(super) fn charge_bytes(&mut self, value: &str) -> Result<(), EvaluationError> {
        self.charge(u64::try_from(value.len()).map_err(|_| EvaluationError::WorkBudget)?)
    }

    pub(super) fn charge(&mut self, units: u64) -> Result<(), EvaluationError> {
        let next = self
            .used
            .checked_add(units)
            .ok_or(EvaluationError::WorkBudget)?;
        if next > 10_000_000 {
            return Err(EvaluationError::WorkBudget);
        }
        self.used = next;
        Ok(())
    }
}

pub(super) struct SortMeter {
    initial: u64,
    remaining: Cell<u64>,
    exhausted: Cell<bool>,
}

impl SortMeter {
    pub(super) fn new(budget: &WorkBudget) -> Self {
        let remaining = 10_000_000 - budget.used;
        Self {
            initial: remaining,
            remaining: Cell::new(remaining),
            exhausted: Cell::new(false),
        }
    }

    fn charge(&self) -> bool {
        if self.exhausted.get() {
            return false;
        }
        if let Some(remaining) = self.remaining.get().checked_sub(1) {
            self.remaining.set(remaining);
            true
        } else {
            self.exhausted.set(true);
            false
        }
    }

    pub(super) fn numbers(&self, left: &f64, right: &f64) -> Ordering {
        if !self.charge() {
            return Ordering::Equal;
        }
        if left == right {
            Ordering::Equal
        } else {
            left.total_cmp(right)
        }
    }

    pub(super) fn strings(&self, left: &str, right: &str) -> Ordering {
        if !self.charge() {
            return Ordering::Equal;
        }
        let left = left.as_bytes();
        let right = right.as_bytes();
        for index in 0..left.len().min(right.len()) {
            if !self.charge() {
                return Ordering::Equal;
            }
            let order = left[index].cmp(&right[index]);
            if order != Ordering::Equal {
                return order;
            }
        }
        left.len().cmp(&right.len())
    }

    pub(super) fn finish(self, budget: &mut WorkBudget) -> Result<(), EvaluationError> {
        budget.charge(self.initial - self.remaining.get())?;
        if self.exhausted.get() {
            Err(EvaluationError::WorkBudget)
        } else {
            Ok(())
        }
    }
}

pub(super) fn check(
    value: &Value,
    depth: usize,
    budget: &mut WorkBudget,
) -> Result<(), EvaluationError> {
    budget.charge(1)?;
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
                check(item, depth + 1, budget)?;
            }
            Ok(())
        }
        Value::Object(items) => {
            if depth >= 128 {
                return Err(EvaluationError::Depth);
            }
            for item in items.values() {
                check(item, depth + 1, budget)?;
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
    let mut budget = WorkBudget::new();
    check(input, 0, &mut budget)?;
    let result = execute(&query.0, input, &mut budget)?;
    check(&result, 0, &mut budget)?;
    Ok(result)
}

fn copy(value: &Value, budget: &mut WorkBudget) -> Result<Value, EvaluationError> {
    check(value, 0, budget)?;
    budget.charge_value(value)?;
    Ok(value.clone())
}

fn push(
    items: &mut Vec<Value>,
    value: Value,
    budget: &mut WorkBudget,
) -> Result<(), EvaluationError> {
    check(&value, 1, budget)?;
    budget.charge(1)?;
    items.push(value);
    Ok(())
}

fn source(node: &Node, input: &Value, budget: &mut WorkBudget) -> Result<Value, EvaluationError> {
    match execute(node, input, budget) {
        Err(error) if !error.is_safety() => Ok(Value::Null),
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

pub(super) fn execute(
    node: &Node,
    input: &Value,
    budget: &mut WorkBudget,
) -> Result<Value, EvaluationError> {
    budget.charge(1)?;
    match node.kind {
        NodeKind::FunctionExpression => {
            let NodeValue::Text(name) = &node.value else {
                return Err(EvaluationError::InvalidNode);
            };
            super::functions_expr::call(name, &node.children, input, budget)
        }
        NodeKind::Comparator => {
            let left = execute(child(node, 0)?, input, budget)?;
            let right = execute(child(node, 1)?, input, budget)?;
            let NodeValue::Comparator(kind) = node.value else {
                return Err(EvaluationError::InvalidNode);
            };
            match kind {
                Kind::Eq => Ok(Value::Bool(budget.equal(&left, &right)?)),
                Kind::Ne => Ok(Value::Bool(!budget.equal(&left, &right)?)),
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
            let left = execute(child(node, 0)?, input, budget)?;
            if is_false(&left) == (node.kind == NodeKind::OrExpression) {
                execute(child(node, 1)?, input, budget)
            } else {
                Ok(left)
            }
        }
        NodeKind::NotExpression => Ok(Value::Bool(is_false(&execute(
            child(node, 0)?,
            input,
            budget,
        )?))),
        NodeKind::Identity | NodeKind::CurrentNode => copy(input, budget),
        NodeKind::Literal => match &node.value {
            NodeValue::Literal(value) => copy(value, budget),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Field => match (&node.value, input) {
            (NodeValue::Text(key), Value::Object(map)) => {
                budget.charge_map_key(key, map.len())?;
                match map.get(key) {
                    Some(value) => copy(value, budget),
                    None => Ok(Value::Null),
                }
            }
            (NodeValue::Text(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Subexpression | NodeKind::IndexExpression | NodeKind::Pipe => {
            let left = execute(child(node, 0)?, input, budget)?;
            execute(child(node, 1)?, &left, budget)
        }
        NodeKind::KeyValPair => execute(child(node, 0)?, input, budget),
        NodeKind::Index => match (&node.value, input) {
            (NodeValue::Index(index), Value::Array(items)) => {
                let size = i128::try_from(items.len()).map_err(|_| EvaluationError::InvalidNode)?;
                let mut index = i128::from(*index);
                if index < 0 {
                    index += size;
                }
                match usize::try_from(index).ok().and_then(|n| items.get(n)) {
                    Some(value) => copy(value, budget),
                    None => Ok(Value::Null),
                }
            }
            (NodeValue::Index(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::Slice => match (&node.value, input) {
            (NodeValue::Slice(parts), Value::Array(items)) => slice(items, *parts, budget),
            (NodeValue::Slice(_), _) => Ok(Value::Null),
            _ => Err(EvaluationError::InvalidNode),
        },
        NodeKind::FilterProjection => {
            let Value::Array(items) = source(child(node, 0)?, input, budget)? else {
                return Ok(Value::Null);
            };
            budget.charge(1)?;
            let mut result = Vec::new();
            for item in &items {
                if !is_false(&execute(child(node, 2)?, item, budget)?) {
                    let value = execute(child(node, 1)?, item, budget)?;
                    if value != Value::Null {
                        push(&mut result, value, budget)?;
                    }
                }
            }
            Ok(Value::Array(result))
        }
        NodeKind::Flatten => {
            let source = source(child(node, 0)?, input, budget)?;
            let Value::Array(items) = source else {
                return Ok(Value::Null);
            };
            budget.charge(1)?;
            let mut result = Vec::new();
            for value in items {
                match value {
                    Value::Array(nested) => {
                        for item in nested {
                            push(&mut result, item, budget)?;
                        }
                    }
                    item => push(&mut result, item, budget)?,
                }
            }
            Ok(Value::Array(result))
        }
        NodeKind::Projection | NodeKind::ValueProjection => {
            let source = if node.kind == NodeKind::ValueProjection {
                source(child(node, 0)?, input, budget)?
            } else {
                execute(child(node, 0)?, input, budget)?
            };
            let mut result = Vec::new();
            match (node.kind, source) {
                (NodeKind::Projection, Value::Array(items)) => {
                    budget.charge(1)?;
                    for item in &items {
                        let value = execute(child(node, 1)?, item, budget)?;
                        if value != Value::Null {
                            push(&mut result, value, budget)?;
                        }
                    }
                }
                (NodeKind::ValueProjection, Value::Object(items)) => {
                    budget.charge(1)?;
                    for item in items.values() {
                        let value = execute(child(node, 1)?, item, budget)?;
                        if value != Value::Null {
                            push(&mut result, value, budget)?;
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
            budget.charge(1)?;
            let mut result = Vec::new();
            for child in &node.children {
                push(&mut result, execute(child, input, budget)?, budget)?;
            }
            Ok(Value::Array(result))
        }
        NodeKind::MultiSelectHash => {
            if *input == Value::Null {
                return Ok(Value::Null);
            }
            budget.charge(1)?;
            let mut result = BTreeMap::new();
            for child in &node.children {
                let NodeValue::Text(key) = &child.value else {
                    return Err(EvaluationError::InvalidNode);
                };
                let value = execute(child, input, budget)?;
                check(&value, 1, budget)?;
                budget.charge(1)?;
                budget.charge_bytes(key)?;
                budget.charge_map_key(key, result.len())?;
                result.insert(key.clone(), value);
            }
            Ok(Value::Object(result))
        }
        _ => Err(EvaluationError::Unsupported),
    }
}

fn slice(
    items: &[Value],
    parts: [Option<i64>; 3],
    budget: &mut WorkBudget,
) -> Result<Value, EvaluationError> {
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
    budget.charge(1)?;
    let mut result = Vec::new();
    while if step < 0 { index > stop } else { index < stop } {
        let position = usize::try_from(index).map_err(|_| EvaluationError::InvalidSlice)?;
        let value = items.get(position).ok_or(EvaluationError::InvalidSlice)?;
        check(value, 1, budget)?;
        budget.charge_value(value)?;
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
    fn recursive_comparisons_charge_string_and_key_bytes() {
        let long = "x".repeat(1_024);
        for value in [
            Value::String(long.clone()),
            Value::Object(BTreeMap::from([(long.clone(), Value::Null)])),
        ] {
            let mut budget = WorkBudget::new();
            budget.charge(9_999_500).unwrap();
            assert_eq!(
                budget.equal(&value, &value),
                Err(EvaluationError::WorkBudget)
            );
        }
    }

    #[test]
    fn multiselect_charges_key_comparisons() {
        let prefix = "x".repeat(1_024);
        let query =
            super::super::compile(&format!("{{\"{prefix}a\": @, \"{prefix}b\": @}}")).unwrap();
        let mut budget = WorkBudget::new();
        budget.charge(9_997_500).unwrap();
        assert_eq!(
            execute(&query.0, &Value::Bool(true), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn object_lookup_charges_key_comparison_bytes() {
        let key = "x".repeat(1_024);
        let query = super::super::compile(&format!("\"{key}\"")).unwrap();
        let input = Value::Object(BTreeMap::from([(key, Value::Null)]));
        let mut budget = WorkBudget::new();
        budget.charge(9_999_500).unwrap();
        assert_eq!(
            execute(&query.0, &input, &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn repeated_contains_with_long_prefixes_hits_budget() {
        let prefix = "x".repeat(64 * 1024);
        let input = Value::Object(BTreeMap::from([
            (
                "items".into(),
                Value::Array(vec![Value::String(format!("{prefix}a")); 64]),
            ),
            ("needle".into(), Value::String(format!("{prefix}b"))),
        ]));
        let query = super::super::compile(
            "contains(items, needle) || contains(items, needle) || contains(items, needle)",
        )
        .unwrap();
        assert!(matches!(
            evaluate(&query, &input),
            Err(EvaluationError::WorkBudget)
        ));
    }

    #[test]
    fn validation_visits_consume_budget() {
        let input = Value::Array(vec![Value::Null; 1_000_000]);
        let mut budget = WorkBudget::new();
        budget.charge(9_000_000).unwrap();
        assert_eq!(
            check(&input, 0, &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn repeated_singleton_validation_stops_at_budget() {
        let expression = vec!["[@]"; 32].join(" | ");
        let query = super::super::compile(&expression).unwrap();
        let input = Value::Array(vec![Value::Null; 1_000_000]);
        assert!(matches!(
            evaluate(&query, &input),
            Err(EvaluationError::WorkBudget)
        ));
    }

    #[test]
    fn large_string_doubling_hits_work_budget() {
        let expression = ["[@,@]"; 8].join(" | ");
        let query = super::super::compile(&expression).unwrap();
        let input = Value::String("x".repeat(1024 * 1024));
        assert!(matches!(
            evaluate(&query, &input),
            Err(EvaluationError::WorkBudget)
        ));
    }

    #[test]
    fn multiselect_key_copy_charges_bytes() {
        let expression = format!("{{\"{}\": @}}", "k".repeat(1_024));
        let query = super::super::compile(&expression).unwrap();
        let mut budget = WorkBudget::new();
        budget.charge(9_999_500).unwrap();
        assert_eq!(
            execute(&query.0, &Value::Bool(true), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }

    #[test]
    fn clone_budget_charges_string_and_key_bytes() {
        let mut budget = WorkBudget::new();
        let value = Value::Object(BTreeMap::from([(
            "abc".into(),
            Value::String("defg".into()),
        )]));
        assert_eq!(budget.charge_value(&value), Ok(()));
        assert_eq!(budget.used, 9);
    }

    #[test]
    fn empty_filter_charges_produced_container() {
        let query = super::super::compile("[?@]").unwrap();
        let mut budget = WorkBudget { used: 9_999_997 };
        assert_eq!(
            execute(&query.0, &Value::Array(vec![]), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn multiselect_charges_produced_elements() {
        let query = super::super::compile("[@,@]").unwrap();
        let mut budget = WorkBudget { used: 9_999_994 };
        assert_eq!(
            execute(&query.0, &Value::Bool(true), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn function_argument_clone_is_metered() {
        let query = super::super::compile("length(@)").unwrap();
        let input = Value::Array(vec![Value::Null, Value::Null]);
        let mut budget = WorkBudget { used: 9_999_994 };
        assert_eq!(
            execute(&query.0, &input, &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn filter_contains_scan_hits_work_budget() {
        let numbers = vec!["0"; 4_000].join(",");
        let expression = format!("[?contains(`[{numbers}]`, @)]");
        let query = super::super::compile(&expression).unwrap();
        let input = Value::Array(vec![Value::Number(1.0); 700]);
        assert_eq!(evaluate(&query, &input), Err(EvaluationError::WorkBudget));
    }
    #[test]
    fn doubling_copy_is_metered() {
        let query = super::super::compile("[@,@] | [@,@]").unwrap();
        let mut budget = WorkBudget { used: 9_999_986 };
        assert_eq!(
            execute(&query.0, &Value::Number(1.0), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn doubling_query_hits_work_budget() {
        let expression = vec!["[@,@]"; 22].join(" | ");
        let query = super::super::compile(&expression).unwrap();
        assert_eq!(
            evaluate(&query, &Value::Number(1.0)),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn budget_charges_each_visited_ast_node() {
        let query = super::super::compile("!@").unwrap();
        let mut budget = WorkBudget { used: 9_999_999 };
        assert_eq!(
            execute(&query.0, &Value::Bool(true), &mut budget),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn budget_counts_recursive_equality_elements() {
        let input = Value::Array(vec![Value::Number(1.0), Value::Array(vec![Value::Null])]);
        let mut budget = WorkBudget::new();
        assert_eq!(budget.equal(&input, &input), Ok(true));
        assert_eq!(budget.used, 4);
        let mut budget = WorkBudget { used: 9_999_998 };
        assert_eq!(
            budget.equal(&input, &input),
            Err(EvaluationError::WorkBudget)
        );
    }
    #[test]
    fn budget_counts_each_nested_value_before_copy() {
        let mut budget = WorkBudget::new();
        let value = Value::Array(vec![
            Value::Number(1.0),
            Value::Object(BTreeMap::from([(
                "k".into(),
                Value::Array(vec![Value::Null]),
            )])),
        ]);
        assert_eq!(budget.charge_value(&value), Ok(()));
        assert_eq!(budget.used, 6);
    }
    #[test]
    fn work_budget_stops_before_exceeding_limit() {
        let mut budget = WorkBudget::new();
        assert_eq!(budget.charge(9_999_999), Ok(()));
        assert_eq!(budget.charge(1), Ok(()));
        assert_eq!(budget.charge(1), Err(EvaluationError::WorkBudget));
        assert_eq!(budget.used, 10_000_000);
        assert_eq!(budget.charge(u64::MAX), Err(EvaluationError::WorkBudget));
    }
    #[test]
    fn work_budget_error_is_never_suppressed() {
        assert!(EvaluationError::WorkBudget.is_safety());
    }
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
            ("merge(`false`)", "FunctionPanic"),
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
        for expression in [
            "(foo(@)).*",
            "(length(`null`)).*",
            "(length(`null`))[]",
            "(length(`null`))[?@]",
        ] {
            assert_eq!(
                evaluate(&super::super::compile(expression).unwrap(), &input),
                Ok(Value::Null)
            );
        }
        for expression in [
            "(contains(`[[]]`, `[]`)).*",
            "(merge(`false`)).*",
            "(&foo).*",
            "(avg(`[]`)).*",
        ] {
            assert!(evaluate(&super::super::compile(expression).unwrap(), &input).is_err());
        }
    }

    #[test]
    fn sorting_does_not_hide_depth_errors_in_source() {
        let deep = (0..126).fold(Value::Null, |value, _| {
            Value::Object(BTreeMap::from([("x".into(), value)]))
        });
        let input = Value::Array(vec![
            Value::Object(BTreeMap::from([("key".into(), Value::Number(1.0))])),
            Value::Object(BTreeMap::from([("deep".into(), deep)])),
        ]);
        let query = super::super::compile("(sort_by(@, &(key || length([[[deep]]])))).*").unwrap();
        assert_eq!(evaluate(&query, &input), Err(EvaluationError::Depth));
    }

    #[test]
    fn function_wrapping_checks_depth_before_outer_function() {
        let nested = |count| {
            (0..count).fold(Value::Null, |value, _| {
                Value::Object(BTreeMap::from([("x".into(), value)]))
            })
        };
        let query = super::super::compile("length(to_array(@))").unwrap();
        assert_eq!(evaluate(&query, &nested(127)), Ok(Value::Number(1.0)));
        assert_eq!(evaluate(&query, &nested(128)), Err(EvaluationError::Depth));
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
