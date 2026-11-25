use std::time::Duration;

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
        operation: &'static str,
        method: HttpMethod,
        route_template: Option<&'static str>,
        status: Option<u16>,
        elapsed: Duration,
    },
    ResponseCapture {
        operation: &'static str,
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
