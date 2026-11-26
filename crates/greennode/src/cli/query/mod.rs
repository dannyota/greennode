#![allow(dead_code)]

mod functions;
mod functions_expr;
mod interpreter;
mod json_size;
mod lexer;
mod literal;
mod parser;

pub(super) type EvaluationError = interpreter::EvaluationError;
pub(super) fn evaluate(
    query: &CompiledQuery,
    input: &crate::cli::render::Value,
) -> Result<crate::cli::render::Value, EvaluationError> {
    interpreter::evaluate(query, input)
}
pub(super) use lexer::SyntaxError;

pub(super) struct CompiledQuery(parser::Node);

pub(super) fn compile(expression: &str) -> Result<CompiledQuery, SyntaxError> {
    parser::parse(expression).map(CompiledQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differential_oracle() {
        let data: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query/differential.json"
        )))
        .unwrap();
        for case in data.as_array().unwrap() {
            if case.get("excluded").is_some() {
                continue;
            }
            let expression = case["expression"].as_str().unwrap();
            let compiled = compile(expression);
            if case["stage"] == "compile" {
                assert!(compiled.is_err(), "{expression}");
                continue;
            }
            let input = crate::cli::render::project(&case["input"])
                .unwrap()
                .for_query()
                .unwrap();
            let actual = evaluate(&compiled.unwrap(), &input);
            if case["stage"] == "success" {
                let expected = crate::cli::render::project(&case["result"])
                    .unwrap()
                    .for_query()
                    .unwrap();
                assert_eq!(actual, Ok(expected), "{expression}");
            } else {
                assert!(actual.is_err(), "{expression}");
            }
        }
    }

    #[test]
    fn compliance_syntax() {
        let directory = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query/compliance"
        );
        let mut total = 0;
        let mut syntax = 0;
        let mut mismatches = Vec::new();
        for file in std::fs::read_dir(directory).unwrap() {
            let path = file.unwrap().path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read_to_string(&path).unwrap();
            let groups: serde_json::Value = serde_json::from_str(&data).unwrap();
            for group in groups.as_array().unwrap() {
                for case in group["cases"].as_array().unwrap() {
                    let expression = case["expression"].as_str().unwrap();
                    let is_syntax = case["error"].as_str() == Some("syntax");
                    total += 1;
                    syntax += usize::from(is_syntax);
                    if compile(expression).is_err() != is_syntax {
                        mismatches.push(format!(
                            "{}: {expression:?} expected syntax={is_syntax}",
                            path.display()
                        ));
                    }
                }
            }
        }
        assert_eq!((total, syntax), (862, 93));
        assert!(
            mismatches.is_empty(),
            "{} mismatches:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }
}
