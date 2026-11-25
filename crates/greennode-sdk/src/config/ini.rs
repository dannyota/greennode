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

fn read_limited(
    mut input: impl Read,
    slot: Slot,
) -> Result<(zeroize::Zeroizing<Vec<u8>>, usize), String> {
    let mut bytes = zeroize::Zeroizing::new(vec![0; 1_048_577]);
    let mut used = 0;
    while used < bytes.len() {
        match input.read(&mut bytes[used..]) {
            Ok(0) => break,
            Ok(count) => used += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(format!("{}: read failed", slot.label())),
        }
    }
    if used > 1_048_576 {
        return Err(format!("{}: file too large", slot.label()));
    }
    Ok((bytes, used))
}

pub(super) fn parse_secrets(input: impl Read) -> Result<SecretsIni, String> {
    let (bytes, used) = read_limited(input, Slot::Credentials)?;
    let mut result = SecretsIni::default();
    let mut section: Option<String> = None;
    for (index, raw) in bytes[..used]
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
    {
        let number = index + 1;
        let error = |reason| format!("credentials:{number}: {reason}");
        if raw.len() > 65_536 || (raw.len() == 65_536 && !raw.ends_with(b"\n")) {
            return Err(error("line too long"));
        }
        let line = std::str::from_utf8(raw).map_err(|_| {
            if index == 0 {
                "credentials: invalid UTF-8".to_owned()
            } else {
                error("invalid UTF-8")
            }
        })?;
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
    let config = matches!(slot, Slot::Config);
    let (bytes, used) = read_limited(input, slot)?;
    let slot = slot.label();
    let mut result = Ini::default();
    let mut section: Option<String> = None;
    for (index, raw) in bytes[..used]
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
    {
        let number = index + 1;
        let error = |reason| format!("{slot}:{number}: {reason}");
        if raw.len() > 65_536 || (raw.len() == 65_536 && !raw.ends_with(b"\n")) {
            return Err(error("line too long"));
        }
        let line = std::str::from_utf8(raw).map_err(|_| {
            if index == 0 {
                format!("{slot}: invalid UTF-8")
            } else {
                error("invalid UTF-8")
            }
        })?;
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
            let value = if config && matches!(key.as_str(), "client_id" | "client_secret") {
                if value.trim().is_empty() {
                    String::new()
                } else {
                    "1".to_owned()
                }
            } else {
                value.trim().to_owned()
            };
            result
                .entry(current.clone())
                .or_default()
                .insert(key, value);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{Slot, parse};

    #[test]
    fn credential_parser_reads_directly_into_one_large_buffer() {
        struct Input {
            read: usize,
            base: Option<usize>,
        }
        impl std::io::Read for Input {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if output.len() < 1024 {
                    return Err(std::io::ErrorKind::InvalidInput.into());
                }
                let base = output.as_ptr() as usize - self.read;
                if self.base.is_some_and(|initial| initial != base) {
                    return Err(std::io::ErrorKind::InvalidInput.into());
                }
                self.base = Some(base);
                let data = b"[default]\npassword=toy\n";
                let size = (data.len() - self.read).min(3);
                output[..size].copy_from_slice(&data[self.read..self.read + size]);
                self.read += size;
                Ok(size)
            }
        }
        let result = super::parse_secrets(Input {
            read: 0,
            base: None,
        });
        assert!(result.is_ok());
        let result = parse(
            Slot::Config,
            Input {
                read: 0,
                base: None,
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn config_parser_masks_reserved_secrets_without_disclosure() {
        let input = b"[default]\nregion=x\nclient_secret=toy-canary\n";
        let parsed = parse(Slot::Config, input.as_slice()).unwrap();
        assert!(!parsed["default"]["client_secret"].is_empty());
        assert!(!parsed["default"]["client_secret"].contains("toy-canary"));
        assert!(!format!("{parsed:?}").contains("toy-canary"));
        let empty = parse(Slot::Config, b"[default]\nclient_secret=  \n".as_slice()).unwrap();
        assert!(empty["default"]["client_secret"].is_empty());
    }

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
    #[test]
    fn read_and_size_errors_have_no_line() {
        struct BadRead;
        impl std::io::Read for BadRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::Other.into())
            }
        }
        for (error, reason) in [
            (parse(Slot::Config, BadRead).err().unwrap(), "read failed"),
            (
                parse(Slot::Config, vec![b'x'; 1_048_577].as_slice())
                    .err()
                    .unwrap(),
                "file too large",
            ),
        ] {
            let detail =
                crate::FileDiagnostic::from_text(crate::FileSlot::Config, Some("option"), &error);
            assert_eq!(detail.reason(), reason);
            assert_eq!(detail.line(), None);
        }
    }
}
