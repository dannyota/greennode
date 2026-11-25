#[allow(dead_code)]
#[path = "../src/event.rs"]
mod event;

use event::{DebugEvent, HttpMethod, OperationId, RouteTemplate};
use std::time::Duration;

#[test]
fn events_expose_only_typed_operation_and_route() {
    let operation = OperationId::new("volume.list");
    let route = RouteTemplate::new("/volumes/{id}");
    let event = DebugEvent::ProductAttempt {
        operation,
        method: HttpMethod::Get,
        route_template: Some(route),
        status: Some(200),
        elapsed: Duration::ZERO,
    };
    match event {
        DebugEvent::ProductAttempt {
            operation,
            method,
            route_template,
            status,
            elapsed,
        } => {
            assert_eq!(operation.to_string(), "volume.list");
            assert_eq!(method, HttpMethod::Get);
            assert_eq!(
                route_template.map(|route| route.to_string()).as_deref(),
                Some("/volumes/{id}")
            );
            assert_eq!(status, Some(200));
            assert_eq!(elapsed, Duration::ZERO);
        }
        DebugEvent::ResponseCapture {
            operation,
            method,
            status,
        } => {
            let _ = (operation, method, status);
            panic!("unexpected event");
        }
        DebugEvent::LoginStarted | DebugEvent::ExchangeStarted => panic!("unexpected event"),
        DebugEvent::LoginFinished { ok } | DebugEvent::ExchangeFinished { ok } => {
            let _ = ok;
            panic!("unexpected event");
        }
    }
}

fn predeclared(source: &str) -> bool {
    ["OperationId::new(", "RouteTemplate::new("]
        .iter()
        .all(|call| {
            source.match_indices(call).all(|(position, _)| {
                let before = &source[..position];
                let statement = before.rsplit([';', '{', '}']).next().unwrap_or("").trim();
                let Some((declaration, initializer)) = statement.split_once('=') else {
                    return false;
                };
                if !initializer.trim().is_empty() {
                    return false;
                }
                let declaration = declaration.trim();
                let declaration = if let Some(rest) = declaration.strip_prefix("pub ") {
                    rest.trim_start()
                } else if let Some(rest) = declaration.strip_prefix("pub(") {
                    let Some((_, rest)) = rest.split_once(')') else {
                        return false;
                    };
                    rest.trim_start()
                } else {
                    declaration
                };
                (declaration.starts_with("const ") || declaration.starts_with("static "))
                    && declaration.contains(':')
            })
        })
}

fn check_directory(path: &std::path::Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_directory(&path);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                predeclared(&source),
                "runtime descriptor constructor in {}",
                path.display()
            );
        }
    }
}

#[test]
fn event_descriptors_are_predeclared_in_sdk_source() {
    assert!(predeclared(
        "const OP: OperationId = OperationId::new(\"op\"); static ROUTE: RouteTemplate = RouteTemplate::new(\"/route\");"
    ));
    assert!(!predeclared(
        "fn f() { let op = OperationId::new(\"op\"); }"
    ));
    assert!(!predeclared(
        "fn f() { let route = RouteTemplate::new(\"route\"); }"
    ));
    check_directory(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}
