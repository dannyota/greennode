use super::Slot;
use std::collections::BTreeMap;
use std::io::Read;

type Ini = BTreeMap<String, BTreeMap<String, String>>;

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
    let mut result = Ini::new();
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
    fn rejects_malformed_lines_without_disclosing_content() {
        for (line, input) in [
            (1, "key = canary\n"),
            (2, "[default]\ncanary\n"),
            (2, "[default]\n = canary\n"),
            (1, "[canary\n"),
            (3, "[default]\nx=y\ncanary\n"),
        ] {
            let err = parse(Slot::Credentials, input.as_bytes()).unwrap_err();
            assert!(err.starts_with(&format!("credentials:{line}:")));
            assert!(!err.contains("canary"));
        }
    }

    #[test]
    fn rejects_invalid_utf8_without_disclosing_content() {
        let err = parse(Slot::Config, &b"[default]\nkey=\xffcanary\n"[..]).unwrap_err();
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
