use std::fmt;
use std::time::Duration;

/// A static identifier for an SDK operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OperationId(&'static str);

crate::error::debug_record!(OperationId(0));

impl OperationId {
    #[allow(dead_code)]
    pub(crate) const fn new(value: &'static str) -> Self {
        Self(value)
    }
    /// Returns the underlying static string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A static route pattern without concrete resource identifiers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RouteTemplate(&'static str);

crate::error::debug_record!(RouteTemplate(0));

impl RouteTemplate {
    #[allow(dead_code)]
    pub(crate) const fn new(value: &'static str) -> Self {
        Self(value)
    }
    /// Returns the underlying static string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for RouteTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// HTTP methods represented in diagnostic events.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PUT.
    Put,
    /// HTTP PATCH.
    Patch,
    /// HTTP DELETE.
    Delete,
    /// HTTP HEAD.
    Head,
    /// HTTP OPTIONS.
    Options,
}

crate::error::debug_record!(HttpMethod; Get, Post, Put, Patch, Delete, Head, Options);

/// Structured diagnostics without request or response bodies.
#[derive(Clone, PartialEq, Eq)]
pub enum DebugEvent {
    /// A product request attempt completed.
    ProductAttempt {
        /// The SDK operation making the request.
        operation: OperationId,
        /// The HTTP method used.
        method: HttpMethod,
        /// The route pattern, when available.
        route_template: Option<RouteTemplate>,
        /// The HTTP status, absent if no response arrived.
        status: Option<u16>,
        /// Time spent on the attempt.
        elapsed: Duration,
    },
    /// A response was captured.
    ResponseCapture {
        /// The SDK operation making the request.
        operation: OperationId,
        /// The HTTP method used.
        method: HttpMethod,
        /// The HTTP response status.
        status: u16,
    },
    /// An IAM login began.
    LoginStarted,
    /// An IAM login finished.
    LoginFinished {
        /// Whether the operation succeeded.
        ok: bool,
    },
    /// A service-account token exchange began.
    ExchangeStarted,
    /// A service-account token exchange finished.
    ExchangeFinished {
        /// Whether the operation succeeded.
        ok: bool,
    },
}

crate::error::debug_record!(DebugEvent [
    ProductAttempt { operation, method, route_template, status, elapsed },
    ResponseCapture { operation, method, status },
    LoginStarted {},
    LoginFinished { ok },
    ExchangeStarted {},
    ExchangeFinished { ok }
]);

/// Receives diagnostic events synchronously.
pub trait DebugEventSink: Send + Sync {
    /// Receives one event.
    fn emit(&self, event: &DebugEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_expose_only_typed_operation_and_route() {
        const OPERATION: OperationId = OperationId::new("volume.list");
        const ROUTE: RouteTemplate = RouteTemplate::new("/volumes/{id}");
        let event = DebugEvent::ProductAttempt {
            operation: OPERATION,
            method: HttpMethod::Get,
            route_template: Some(ROUTE),
            status: Some(200),
            elapsed: Duration::ZERO,
        };
        assert_eq!(format!("{event:x?}"), format!("{event:?}"));
        assert_eq!(format!("{event:06x?}"), format!("{event:?}"));
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
}
