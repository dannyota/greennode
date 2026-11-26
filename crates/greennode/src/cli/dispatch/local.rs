use std::borrow::Cow;

pub struct LocalError {
    pub code: &'static str,
    pub message: Cow<'static, str>,
    pub exit: u8,
}

impl LocalError {
    pub fn usage(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code: "InvalidUsage",
            message: message.into(),
            exit: 2,
        }
    }

    pub fn read_only(source: &'static str) -> Self {
        Self {
            code: "ReadOnly",
            message: format!("read-only: refused by {source}").into(),
            exit: 2,
        }
    }

    pub(super) fn missing_input() -> Self {
        Self {
            code: "RequestFailed",
            message: "operation input unavailable".into(),
            exit: 1,
        }
    }
}
