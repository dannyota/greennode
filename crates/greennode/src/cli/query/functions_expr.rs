// Translated from go-jmespath v0.4.0, Copyright 2015 James Saryerwinnie, Apache-2.0; changed.

use super::{
    interpreter::{EvaluationError, check, execute},
    parser::{Node, NodeKind},
};
use crate::cli::render::Value;

enum Argument<'a> {
    Value(Value),
    Reference(&'a Node),
}

pub(super) fn call(name: &str, nodes: &[Node], input: &Value) -> Result<Value, EvaluationError> {
    let mut arguments = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.kind == NodeKind::ExpRef {
            arguments.push(Argument::Reference(
                node.children.first().ok_or(EvaluationError::InvalidNode)?,
            ));
        } else {
            arguments.push(Argument::Value(execute(node, input)?));
        }
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
        for value in values {
            let item = execute(reference, value)?;
            check(&item, 1)?;
            result.push(item);
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
    let key = execute(reference, &values[0])?;
    let numeric = match key {
        Value::Number(_) => true,
        Value::String(_) => false,
        _ => return Err(EvaluationError::FunctionType),
    };
    if name != "sort_by" {
        let mut best = &values[0];
        let mut best_key = key;
        for value in &values[1..] {
            let current = execute(reference, value)?;
            let ordering =
                compare_keys(&current, &best_key, numeric).ok_or(EvaluationError::FunctionType)?;
            if (name == "max_by" && ordering.is_gt()) || (name == "min_by" && ordering.is_lt()) {
                best = value;
                best_key = current;
            }
        }
        return Ok(best.clone());
    }
    let mut sorted = values.clone();
    let mut sorter = Sorter {
        values: &mut sorted,
        reference,
        numeric,
        failed: false,
    };
    sorter.sort();
    if sorter.failed {
        return Err(EvaluationError::FunctionComparison);
    }
    Ok(Value::Array(sorted))
}

fn compare_keys(a: &Value, b: &Value, numeric: bool) -> Option<std::cmp::Ordering> {
    match (a, b, numeric) {
        (Value::Number(a), Value::Number(b), true) => a.partial_cmp(b),
        (Value::String(a), Value::String(b), false) => Some(a.cmp(b)),
        _ => None,
    }
}

struct Sorter<'a> {
    values: &'a mut [Value],
    reference: &'a Node,
    numeric: bool,
    failed: bool,
}
impl Sorter<'_> {
    fn less(&mut self, i: usize, j: usize) -> bool {
        let result = execute(self.reference, &self.values[i]).and_then(|a| {
            execute(self.reference, &self.values[j]).and_then(|b| {
                compare_keys(&a, &b, self.numeric).ok_or(EvaluationError::FunctionType)
            })
        });
        match result {
            Ok(order) => order.is_lt(),
            Err(_) => {
                self.failed = true;
                true
            }
        }
    }
    fn insertion(&mut self, a: usize, b: usize) {
        for i in a + 1..b {
            let mut j = i;
            while j > a && self.less(j, j - 1) {
                self.values.swap(j, j - 1);
                j -= 1;
            }
        }
    }
    fn rotate(&mut self, a: usize, m: usize, b: usize) {
        let (mut i, mut j) = (m - a, b - m);
        while i != j {
            if i > j {
                for k in 0..j {
                    self.values.swap(m - i + k, m + k);
                }
                i -= j;
            } else {
                for k in 0..i {
                    self.values.swap(m - i + k, m + j - i + k);
                }
                j -= i;
            }
        }
        for k in 0..i {
            self.values.swap(m - i + k, m + k);
        }
    }
    fn merge(&mut self, a: usize, m: usize, b: usize) {
        if m - a == 1 {
            let (mut i, mut j) = (m, b);
            while i < j {
                let h = i + (j - i) / 2;
                if self.less(h, a) {
                    i = h + 1;
                } else {
                    j = h;
                }
            }
            for k in a..i - 1 {
                self.values.swap(k, k + 1);
            }
            return;
        }
        if b - m == 1 {
            let (mut i, mut j) = (a, m);
            while i < j {
                let h = i + (j - i) / 2;
                if !self.less(m, h) {
                    i = h + 1;
                } else {
                    j = h;
                }
            }
            for k in (i + 1..=m).rev() {
                self.values.swap(k, k - 1);
            }
            return;
        }
        let mid = a + (b - a) / 2;
        let n = mid + m;
        let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
        let p = n - 1;
        while start < r {
            let c = start + (r - start) / 2;
            if !self.less(p - c, c) {
                start = c + 1;
            } else {
                r = c;
            }
        }
        let end = n - start;
        if start < m && m < end {
            self.rotate(start, m, end);
        }
        if a < start && start < mid {
            self.merge(a, start, mid);
        }
        if mid < end && end < b {
            self.merge(mid, end, b);
        }
    }
    fn sort(&mut self) {
        let n = self.values.len();
        let mut a = 0;
        while a + 20 <= n {
            self.insertion(a, a + 20);
            a += 20;
        }
        self.insertion(a, n);
        let mut block = 20;
        while block < n {
            let mut a = 0;
            while a + 2 * block <= n {
                self.merge(a, a + block, a + 2 * block);
                a += 2 * block;
            }
            if a + block < n {
                self.merge(a, a + block, n);
            }
            block *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{compile, evaluate};
    use crate::cli::render::Value;

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
