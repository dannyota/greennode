use std::fmt;
use std::time::Duration;

/// A static identifier for an SDK operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationId(&'static str);

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteTemplate(&'static str);

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Structured diagnostics without request or response bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Receives diagnostic events synchronously.
pub trait DebugEventSink: Send + Sync {
    /// Receives one event.
    fn emit(&self, event: &DebugEvent);
}
