use super::Slot;
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Default)]
pub(super) struct Ini(BTreeMap<String, BTreeMap<String, String>>);

impl std::fmt::Debug for Ini {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Ini([REDACTED])")
    }
}

impl std::ops::Deref for Ini {
    type Target = BTreeMap<String, BTreeMap<String, String>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Ini {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default)]
pub(super) struct SecretsIni(pub BTreeMap<String, BTreeMap<String, crate::Secret>>);

impl std::fmt::Debug for SecretsIni {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretsIni([REDACTED])")
    }
}

pub(super) fn parse_secrets(input: impl Read) -> Result<SecretsIni, String> {
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    input
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "credentials:1: read failed".to_owned())?;
    if bytes.len() > 1_048_576 {
        return Err("credentials:1: file too large".into());
    }
    let mut result = SecretsIni::default();
    let mut section: Option<String> = None;
    for (index, raw) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let number = index + 1;
        let error = |reason| format!("credentials:{number}: {reason}");
        if raw.len() > 65_536 || (raw.len() == 65_536 && !raw.ends_with(b"\n")) {
            return Err(error("line too long"));
        }
        let line = std::str::from_utf8(raw).map_err(|_| error("invalid UTF-8"))?;
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(header) = trimmed.strip_prefix('[') {
            let name = header
                .strip_suffix(']')
                .ok_or_else(|| error("unclosed section header"))?
                .trim()
                .to_owned();
            result.0.entry(name.clone()).or_default();
            section = Some(name);
        } else {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| error("line has no '='"))?;
            let current = section
                .as_ref()
                .ok_or_else(|| error("key before any section"))?;
            let key: String = key
                .trim()
                .chars()
                .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
                .collect();
            if key.is_empty() {
                return Err(error("empty key"));
            }
            result
                .0
                .entry(current.clone())
                .or_default()
                .insert(key, crate::Secret::new(value.trim().to_owned()));
        }
    }
    Ok(result)
}

pub(super) fn parse(slot: Slot, input: impl Read) -> Result<Ini, String> {
    let slot = slot.label();
    let mut bytes = Vec::new();
    input
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{slot}:1: read failed"))?;
    if bytes.len() > 1_048_576 {
        return Err(format!("{slot}:1: file too large"));
    }
    let mut result = Ini::default();
    let mut section: Option<String> = None;
    for (index, raw) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let number = index + 1;
        let error = |reason| format!("{slot}:{number}: {reason}");
        if raw.len() > 65_536 || (raw.len() == 65_536 && !raw.ends_with(b"\n")) {
            return Err(error("line too long"));
        }
        let line = std::str::from_utf8(raw).map_err(|_| error("invalid UTF-8"))?;
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(header) = trimmed.strip_prefix('[') {
            let name = header
                .strip_suffix(']')
                .ok_or_else(|| error("unclosed section header"))?
                .trim()
                .to_owned();
            result.entry(name.clone()).or_default();
            section = Some(name);
        } else {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| error("line has no '='"))?;
            let current = section
                .as_ref()
                .ok_or_else(|| error("key before any section"))?;
            let key: String = key
                .trim()
                .chars()
                .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
                .collect();
            if key.is_empty() {
                return Err(error("empty key"));
            }
            result
                .entry(current.clone())
                .or_default()
                .insert(key, value.trim().to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{Slot, parse};

    #[test]
    fn credential_parser_preserves_baseline_grammar_and_bounds() {
        let input = "\u{feff}[default]\nPassword=first\n[other]\nx=y\n[default]\nPASSWORD=last\n";
        let parsed = super::parse_secrets(input.as_bytes()).unwrap();
        assert!(parsed.0["default"]["password"].expose_secret() == "last");
        let valid = format!("[x]\nk={}\n", "a".repeat(65533));
        let invalid = format!("[x]\nk={}\n", "a".repeat(65534));
        assert!(super::parse_secrets(valid.as_bytes()).is_ok());
        assert!(super::parse_secrets(invalid.as_bytes()).is_err());
        let line = format!("#{}\n", "a".repeat(65534));
        let exact = line.repeat(16);
        assert!(super::parse_secrets(exact.as_bytes()).is_ok());
        assert!(super::parse_secrets(format!("{exact}\n").as_bytes()).is_err());
    }

    #[test]
    fn credential_parser_redacts_values_and_errors() {
        let parsed = super::parse_secrets(b"[default]\npassword=toy-canary\n".as_slice());
        assert!(parsed.is_ok());
        let parsed = parsed.unwrap();
        assert!(parsed.0["default"]["password"].expose_secret() == "toy-canary");
        assert!(!format!("{parsed:?}").contains("toy-canary"));
        let error = super::parse_secrets(b"[default]\nx=ok\ntoy-canary\n".as_slice())
            .err()
            .unwrap();
        assert!(error.starts_with("credentials:3:"));
        assert!(!error.contains("toy-canary"));
    }

    #[test]
    fn parses_baseline_sections_and_values() {
        let input = "\u{feff}# comment\n[ default ]\nRegion = first\n; comment\nkey = a = b  c\n[default]\nregion = last\nempty =\n[profile dev]\nregion = other\n";
        let ini = parse(Slot::Config, input.as_bytes()).unwrap();
        assert_eq!(ini["default"]["region"], "last");
        assert_eq!(ini["default"]["key"], "a = b  c");
        assert_eq!(ini["default"]["empty"], "");
        assert_eq!(ini["profile dev"]["region"], "other");
        assert!(!ini.contains_key("missing"));
    }

    #[test]
    fn lowercases_unicode_keys_like_baseline() {
        let parsed = parse(Slot::Config, "[x]\nİ = value\nUPPER = ok\n".as_bytes()).unwrap();
        assert!(parsed["x"].contains_key("i"));
        assert!(parsed["x"].contains_key("upper"));
    }

    #[test]
    fn debug_redacts_parsed_values() {
        let parsed = parse(Slot::Credentials, b"[default]\nkey=canary\n".as_slice()).unwrap();
        assert!(!format!("{parsed:?}").contains("canary"));
        assert!(parsed["default"]["key"] == "canary", "parsed value changed");
    }

    #[test]
    fn rejects_malformed_lines_without_disclosing_content() {
        for (line, input) in [
            (1, "key = canary\n"),
            (2, "[default]\ncanary\n"),
            (2, "[default]\n = canary\n"),
            (1, "[canary\n"),
            (3, "[default]\nx=y\ncanary\n"),
        ] {
            let err = parse(Slot::Credentials, input.as_bytes()).err().unwrap();
            assert!(err.starts_with(&format!("credentials:{line}:")));
            assert!(!err.contains("canary"));
        }
    }

    #[test]
    fn rejects_invalid_utf8_without_disclosing_content() {
        let err = parse(Slot::Config, &b"[default]\nkey=\xffcanary\n"[..])
            .err()
            .unwrap();
        assert!(err.starts_with("config:2:"));
        assert!(!err.contains("canary"));
        assert!(parse(Slot::Config, &b"[default]\nkey=ok\n"[..]).is_ok());
    }

    #[test]
    fn bounds_lines_including_terminator() {
        let valid = format!("[x]\nk={}\n", "a".repeat(65533));
        let invalid = format!("[x]\nk={}\n", "a".repeat(65534));
        assert!(parse(Slot::Config, valid.as_bytes()).is_ok());
        assert!(parse(Slot::Config, invalid.as_bytes()).is_err());
        let valid_final_line = format!("[x]\nk={}", "a".repeat(65533));
        assert!(parse(Slot::Config, valid_final_line.as_bytes()).is_ok());
        let final_line = format!("[x]\nk={}", "a".repeat(65534));
        assert!(parse(Slot::Config, final_line.as_bytes()).is_err());
    }

    #[test]
    fn reads_at_most_one_byte_past_limit() {
        use std::cell::Cell;
        use std::io::{self, Read};
        use std::rc::Rc;

        struct Input(Rc<Cell<usize>>);
        impl Read for Input {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let remaining = 2_097_152usize.saturating_sub(self.0.get());
                let count = remaining.min(buf.len());
                buf[..count].fill(b'\n');
                self.0.set(self.0.get() + count);
                Ok(count)
            }
        }
        let consumed = Rc::new(Cell::new(0));
        assert!(parse(Slot::Config, Input(consumed.clone())).is_err());
        assert_eq!(consumed.get(), 1_048_577);
    }

    #[test]
    fn bounds_total_input() {
        let line = format!("#{}\n", "a".repeat(65534));
        let exact = line.repeat(16);
        assert_eq!(exact.len(), 1024 * 1024);
        assert!(parse(Slot::Config, exact.as_bytes()).is_ok());
        assert!(parse(Slot::Config, format!("{exact}\n").as_bytes()).is_err());
    }
}
