use std::collections::BTreeMap;
use std::io::Write;

use super::value::{cell, check_depth, escape, rows};
use super::{Error, Value};

fn measure(
    sizes: &mut BTreeMap<String, (usize, usize)>,
    key: &str,
    value: &Value,
) -> Result<(), Error> {
    let header = escape(key);
    let header_width = header.chars().count();
    let header_extra = header
        .len()
        .checked_sub(header_width)
        .ok_or(Error::TableLimit)?;
    let size = sizes
        .entry(key.to_owned())
        .or_insert((header_width, header_extra));
    let text = cell(value)?;
    let width = text.chars().count();
    size.0 = size.0.max(width);
    size.1 = size
        .1
        .max(text.len().checked_sub(width).ok_or(Error::TableLimit)?);
    if sizes.len() > 100 {
        return Err(Error::TableLimit);
    }
    Ok(())
}

fn border<W: Write>(w: &mut W, sizes: &BTreeMap<String, (usize, usize)>) -> Result<(), Error> {
    write!(w, "+")?;
    for (width, _) in sizes.values() {
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
    check_depth(value)?;
    let items = rows(value);
    let mut sizes = BTreeMap::new();
    for item in &items {
        match item {
            Value::Object(map) => {
                for (key, value) in map {
                    measure(&mut sizes, key, value)?;
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
                Value::Object(map) => match map.get(key) {
                    Some(v) => cell(v)?,
                    None => String::new(),
                },
                _ if key == "Value" => cell(item)?,
                _ => String::new(),
            };
            cells.push((text, *width));
        }
        row(w, cells.into_iter())?;
    }
    border(w, &sizes)
}
