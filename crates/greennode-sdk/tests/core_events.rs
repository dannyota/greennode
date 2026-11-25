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
    if let DebugEvent::ProductAttempt {
        operation,
        route_template: Some(route),
        ..
    } = event
    {
        assert_eq!(operation.to_string(), "volume.list");
        assert_eq!(route.to_string(), "/volumes/{id}");
    } else {
        panic!("unexpected event");
    }
}
