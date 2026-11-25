use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationId(&'static str);

impl OperationId {
    #[allow(dead_code)]
    pub(crate) const fn new(value: &'static str) -> Self {
        Self(value)
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteTemplate(&'static str);

impl RouteTemplate {
    #[allow(dead_code)]
    pub(crate) const fn new(value: &'static str) -> Self {
        Self(value)
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for RouteTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DebugEvent {
    ProductAttempt {
        operation: OperationId,
        method: HttpMethod,
        route_template: Option<RouteTemplate>,
        status: Option<u16>,
        elapsed: Duration,
    },
    ResponseCapture {
        operation: OperationId,
        method: HttpMethod,
        status: u16,
    },
    LoginStarted,
    LoginFinished {
        ok: bool,
    },
    ExchangeStarted,
    ExchangeFinished {
        ok: bool,
    },
}

pub trait DebugEventSink: Send + Sync {
    fn emit(&self, event: &DebugEvent);
}
