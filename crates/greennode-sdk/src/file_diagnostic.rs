/// Identifies which configuration file produced a diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileSlot {
    /// The settings file.
    Config,
    /// The credentials file.
    Credentials,
}

/// Value-free details of a configuration file failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiagnostic {
    slot: FileSlot,
    source: &'static str,
    reason: &'static str,
    line: Option<u32>,
}

impl FileDiagnostic {
    /// Returns the affected file slot.
    pub fn slot(&self) -> FileSlot {
        self.slot
    }
    /// Returns the fixed source label.
    pub fn source(&self) -> &'static str {
        self.source
    }
    /// Returns the fixed failure reason.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    /// Returns the line number when the failure occurred while parsing.
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    pub(crate) fn from_text(slot: FileSlot, source: Option<&'static str>, text: &str) -> Self {
        let source = match source {
            Some("option") => "option",
            Some("GREENNODE_CONFIG_FILE") => "GREENNODE_CONFIG_FILE",
            Some("GREENNODE_SHARED_CREDENTIALS_FILE") => "GREENNODE_SHARED_CREDENTIALS_FILE",
            Some("VNGCLOUD_CONFIG_FILE") => "VNGCLOUD_CONFIG_FILE",
            Some("VNGCLOUD_SHARED_CREDENTIALS_FILE") => "VNGCLOUD_SHARED_CREDENTIALS_FILE",
            Some("~/.greennode") => "~/.greennode",
            Some("~/.vngcloud") => "~/.vngcloud",
            _ => "unknown",
        };
        let mut parts = text.splitn(3, ':');
        let _slot = parts.next();
        let second = parts.next().unwrap_or("").trim();
        let (line, value) = match second.parse::<u32>() {
            Ok(line) => (Some(line), parts.next().unwrap_or("").trim()),
            Err(_) => (None, second),
        };
        let reason = match value {
            "lookup failed" => "lookup failed",
            "invalid path" => "invalid path",
            "not found" => "not found",
            "open failed" => "open failed",
            "metadata failed" => "metadata failed",
            "not a regular file" => "not a regular file",
            "file too large" => "file too large",
            "unsafe permissions; use chmod 600" => "unsafe permissions; use chmod 600",
            "read failed" => "read failed",
            "line too long" => "line too long",
            "invalid UTF-8" => "invalid UTF-8",
            "unclosed section header" => "unclosed section header",
            "line has no '='" => "line has no '='",
            "key before any section" => "key before any section",
            "empty key" => "empty key",
            _ => "unknown error",
        };
        Self {
            slot,
            source,
            reason,
            line,
        }
    }
}
