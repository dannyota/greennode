use std::io::{self, Write};

use super::{Result, TargetSlot, files::FILE_LIMIT};

const BOM: &str = "\u{feff}";
const LINE_LIMIT: usize = 65_536;

pub(super) struct Edit<'a> {
    before: &'a str,
    after: &'a str,
    leading_breaks: usize,
    section: Option<&'a str>,
    key: Option<&'a str>,
    value: &'a str,
    ending: &'a str,
}

impl std::fmt::Debug for Edit<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Edit([redacted])")
    }
}

impl<'a> Edit<'a> {
    pub(super) fn new(
        bytes: &'a [u8],
        slot: TargetSlot,
        section: &'a str,
        key: &'a str,
        value: &'a str,
    ) -> Result<Self> {
        let content = std::str::from_utf8(bytes).map_err(|error| {
            let line = bytes
                .get(..error.valid_up_to())
                .map_or(1, |valid| 1 + valid.iter().filter(|&&b| b == b'\n').count());
            slot.error("invalid UTF-8", Some(line))
        })?;
        let body = content.strip_prefix(BOM).unwrap_or(content);
        let mut current = None;
        let mut insertion = None;
        let mut replacement = None;
        let mut offset = content.len() - body.len();
        let mut last_trimmed = "";
        let mut ending = "\n";
        for (index, line) in body.split_inclusive('\n').enumerate() {
            let fail = |reason| slot.error(reason, Some(index + 1));
            let next = offset + line.len();
            if line.ends_with("\r\n") {
                ending = "\r\n";
            } else if line.ends_with('\n') {
                ending = "\n";
            }
            let parsed_len = if index == 0 { next } else { line.len() };
            if parsed_len > LINE_LIMIT || (parsed_len == LINE_LIMIT && !line.ends_with('\n')) {
                return Err(fail("line too long"));
            }
            let trimmed = line.trim();
            last_trimmed = trimmed;
            if let Some(header) = trimmed.strip_prefix('[') {
                current = Some(
                    header
                        .strip_suffix(']')
                        .ok_or_else(|| fail("unclosed section header"))?
                        .trim(),
                );
                if current == Some(section) {
                    insertion = Some((next, ending));
                }
            } else if !trimmed.is_empty() && !trimmed.starts_with(['#', ';']) {
                let (name, old_value) = line
                    .split_once('=')
                    .ok_or_else(|| fail("line has no '='"))?;
                let name_trimmed = name.trim();
                if name_trimmed.is_empty() {
                    return Err(fail("empty key"));
                }
                let current = current.ok_or_else(|| fail("key before any section"))?;
                if current == section {
                    insertion = Some((next, ending));
                    let matches = name_trimmed
                        .chars()
                        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
                        .eq(key.chars());
                    if matches {
                        let terminator = if old_value.ends_with("\r\n") {
                            "\r\n"
                        } else {
                            "\n"
                        };
                        let start = offset + name.len() + 1;
                        let prefix_len = start - if index == 0 { 0 } else { offset };
                        replacement = Some((start, next, terminator, prefix_len, index + 1));
                    }
                }
            }
            offset = next;
        }

        let edit = if let Some((start, end, ending, prefix_len, number)) = replacement {
            if prefix_len
                .saturating_add(1 + ending.len())
                .saturating_add(value.len())
                > LINE_LIMIT
            {
                return Err(slot.error("edited line too long", Some(number)));
            }
            Self {
                before: content
                    .get(..start)
                    .ok_or_else(|| slot.error("invalid edit offset", None))?,
                after: content
                    .get(end..)
                    .ok_or_else(|| slot.error("invalid edit offset", None))?,
                leading_breaks: 0,
                section: None,
                key: None,
                value,
                ending,
            }
        } else {
            let (offset, ending) = insertion.unwrap_or((content.len(), ending));
            let (before, after) = content
                .split_at_checked(offset)
                .ok_or_else(|| slot.error("invalid edit offset", None))?;
            let needs_break = !before.is_empty() && before != BOM && !before.ends_with('\n');
            let section = insertion.is_none().then_some(section);
            let needs_blank = section.is_some() && !last_trimmed.is_empty();
            let leading_breaks = usize::from(needs_break) + usize::from(needs_blank);
            if needs_break
                && before.rsplit('\n').next().unwrap_or("").len() + ending.len() > LINE_LIMIT
            {
                return Err(slot.error("edited line too long", None));
            }
            if key
                .len()
                .saturating_add(3)
                .saturating_add(value.len())
                .saturating_add(ending.len())
                > LINE_LIMIT
            {
                return Err(slot.error("edited line too long", None));
            }
            if section.is_some_and(|s| s.len().saturating_add(2 + ending.len()) > LINE_LIMIT) {
                return Err(slot.error("edited section header too long", None));
            }
            Self {
                before,
                after,
                leading_breaks,
                section,
                key: Some(key),
                value,
                ending,
            }
        };
        if edit.len() > FILE_LIMIT {
            return Err(slot.error("edited file too large", None));
        }
        Ok(edit)
    }

    fn len(&self) -> usize {
        let header = self
            .section
            .map_or(0, |section| section.len() + 2 + self.ending.len());
        let key = self.key.map_or(1, |key| key.len() + 3);
        self.before.len()
            + self.leading_breaks * self.ending.len()
            + header
            + key
            + self.value.len()
            + self.ending.len()
            + self.after.len()
    }

    pub(super) fn write(&self, file: &mut impl Write) -> io::Result<()> {
        file.write_all(self.before.as_bytes())?;
        for _ in 0..self.leading_breaks {
            file.write_all(self.ending.as_bytes())?;
        }
        if let Some(section) = self.section {
            write!(file, "[{section}]{}", self.ending)?;
        }
        if let Some(key) = self.key {
            write!(file, "{key} = ")?;
        } else {
            file.write_all(b" ")?;
        }
        file.write_all(self.value.as_bytes())?;
        file.write_all(self.ending.as_bytes())?;
        file.write_all(self.after.as_bytes())
    }
}
