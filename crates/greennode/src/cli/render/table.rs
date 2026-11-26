use std::collections::BTreeMap;
use std::io::Write;

use super::value::{cell, check_depth, escape, escape_write, rows, write_cell};
use super::{Error, Value};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderers_refuse_deep_tuple_variant_tree() {
        let mut value = Value::Null;
        for _ in 0..65 {
            value = Value::Object(BTreeMap::from([("Next".into(), Value::Array(vec![value]))]));
        }
        assert!(matches!(
            write_table(&mut Vec::new(), &value),
            Err(Error::DepthLimit)
        ));
        assert!(matches!(
            super::super::write_text(&mut Vec::new(), &value),
            Err(Error::DepthLimit)
        ));
    }

    #[test]
    fn preflight_borrows_headers() {
        let key = "header".repeat(10000);
        let mut sizes = BTreeMap::new();
        measure(&mut sizes, &key, &Value::Null).unwrap();
        let stored = sizes.keys().next().unwrap();
        assert!(std::ptr::eq(stored.as_ptr(), key.as_ptr()));
    }

    #[test]
    fn preflight_refuses_large_escaped_cell() {
        let input = Value::String("\0".repeat(32 * 1024 * 1024));
        assert!(matches!(measure_cell(&input), Err(Error::TableLimit)));
        assert!(matches!(
            write_table(&mut Vec::new(), &input),
            Err(Error::TableLimit)
        ));
    }
}

#[derive(Default)]
struct Counter {
    bytes: usize,
    width: usize,
    exceeded: bool,
}

impl Write for Counter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let bytes = self
            .bytes
            .checked_add(data.len())
            .ok_or_else(|| std::io::Error::other("limit"))?;
        if bytes > 64 * 1024 * 1024 {
            self.exceeded = true;
            return Err(std::io::Error::other("limit"));
        }
        let text = std::str::from_utf8(data).map_err(|_| std::io::Error::other("invalid UTF-8"))?;
        self.width = self
            .width
            .checked_add(text.chars().count())
            .ok_or_else(|| std::io::Error::other("limit"))?;
        self.bytes = bytes;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn measure_cell(value: &Value) -> Result<(usize, usize), Error> {
    let mut counter = Counter::default();
    let result = write_cell(&mut counter, value);
    if counter.exceeded {
        return Err(Error::TableLimit);
    }
    result?;
    Ok((
        counter.width,
        counter
            .bytes
            .checked_sub(counter.width)
            .ok_or(Error::TableLimit)?,
    ))
}

fn measure<'a>(
    sizes: &mut BTreeMap<&'a str, (usize, usize)>,
    key: &'a str,
    value: &Value,
) -> Result<(), Error> {
    let mut header = Counter::default();
    let result = escape_write(&mut header, key);
    if header.exceeded {
        return Err(Error::TableLimit);
    }
    result?;
    let header_extra = header
        .bytes
        .checked_sub(header.width)
        .ok_or(Error::TableLimit)?;
    let (width, extra) = measure_cell(value)?;
    let size = sizes.entry(key).or_insert((header.width, header_extra));
    size.0 = size.0.max(width);
    size.1 = size.1.max(extra);
    if sizes.len() > 100 {
        return Err(Error::TableLimit);
    }
    Ok(())
}

fn border<W: Write>(w: &mut W, sizes: &[(&str, (usize, usize))]) -> Result<(), Error> {
    write!(w, "+")?;
    for (_, (width, _)) in sizes {
        let length = width.checked_add(2).ok_or(Error::TableLimit)?;
        write!(w, "{}+", "-".repeat(length))?;
    }
    writeln!(w)?;
    Ok(())
}

fn row<W: Write>(w: &mut W, cells: impl Iterator<Item = (String, usize)>) -> Result<(), Error> {
    write!(w, "|")?;
    for (text, width) in cells {
        let pad = width
            .checked_sub(text.chars().count())
            .ok_or(Error::TableLimit)?;
        write!(w, " {text}{} |", " ".repeat(pad))?;
    }
    writeln!(w)?;
    Ok(())
}

pub fn write_table<W: Write>(w: &mut W, value: &Value) -> Result<(), Error> {
    write_table_with_columns(w, value, None)
}

#[derive(Clone, Copy)]
pub(crate) struct TableView {
    pub(crate) columns: &'static [&'static str],
    pub(crate) show_empty_headers: bool,
}

pub(crate) fn write_table_with_columns<W: Write>(
    w: &mut W,
    value: &Value,
    view: Option<TableView>,
) -> Result<(), Error> {
    check_depth(value)?;
    let items = rows(value);
    let columns = view
        .filter(|view| view.show_empty_headers || !items.is_empty())
        .map(|view| view.columns);
    let mut sizes = BTreeMap::new();
    if let Some(columns) = columns {
        for key in columns {
            measure(&mut sizes, key, &Value::Null)?;
        }
    }
    for item in items {
        match item {
            Value::Object(map) => {
                for (key, value) in map {
                    if columns.is_none_or(|columns| columns.contains(&key.as_str())) {
                        measure(&mut sizes, key, value)?;
                    }
                }
            }
            _ => measure(&mut sizes, "Value", item)?,
        }
    }
    if sizes.is_empty() && items.is_empty() {
        measure(&mut sizes, "Value", &Value::Null)?;
    }
    let count = items
        .len()
        .checked_mul(sizes.len())
        .ok_or(Error::TableLimit)?;
    if count > 1_000_000 {
        return Err(Error::TableLimit);
    }
    let mut row_bytes = sizes
        .len()
        .checked_mul(3)
        .and_then(|n| n.checked_add(2))
        .ok_or(Error::TableLimit)?;
    for (width, extra) in sizes.values() {
        row_bytes = row_bytes
            .checked_add(*width)
            .and_then(|n| n.checked_add(*extra))
            .ok_or(Error::TableLimit)?;
    }
    let projected = items
        .len()
        .checked_add(4)
        .and_then(|n| n.checked_mul(row_bytes))
        .ok_or(Error::TableLimit)?;
    if projected > 64 * 1024 * 1024 {
        return Err(Error::TableLimit);
    }
    let sizes: Vec<_> = match columns {
        Some(columns) => columns
            .iter()
            .filter_map(|&key| sizes.get(key).map(|&size| (key, size)))
            .collect(),
        None => sizes.into_iter().collect(),
    };
    border(w, &sizes)?;
    row(
        w,
        sizes.iter().map(|(key, (width, _))| (escape(key), *width)),
    )?;
    border(w, &sizes)?;
    for item in items {
        let mut cells = Vec::new();
        for (key, (width, _)) in &sizes {
            let text = match item {
                Value::Object(map) => match map.get(*key) {
                    Some(v) => cell(v)?,
                    None => String::new(),
                },
                _ if *key == "Value" => cell(item)?,
                _ => String::new(),
            };
            cells.push((text, *width));
        }
        row(w, cells.into_iter())?;
    }
    border(w, &sizes)
}
