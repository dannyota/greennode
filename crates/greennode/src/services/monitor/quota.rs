use std::collections::BTreeMap;
use std::io::Write;

use crate::cli::dispatch::{Format, OutputView};
use crate::cli::render::{self, Value};

type Fields = BTreeMap<String, Value>;

pub(super) fn catalog_view() -> OutputView {
    OutputView {
        custom: Some(render_catalog),
        transform_query_cells: Some(unknown_cells),
        ..OutputView::default()
    }
}

pub(super) fn status_view() -> OutputView {
    OutputView {
        custom: Some(render_status),
        transform_query_cells: Some(unknown_cells),
        use_for_partial: true,
        ..OutputView::default()
    }
}

fn render_catalog(
    mut stdout: &mut dyn Write,
    _: &mut dyn Write,
    output: &render::Projected,
    format: Format,
    queried: bool,
) -> Result<bool, render::Error> {
    if format == Format::Json || queried {
        return Ok(false);
    }
    let mut exact = Vec::new();
    render::write_json(&mut exact, output)?;
    let exact: serde_json::Value = serde_json::from_slice(&exact)
        .map_err(|error| render::Error::Invalid(error.to_string()))?;
    let exact_scalar = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        _ => "unknown".to_owned(),
    };
    let periods = exact["MonthPeriods"]
        .as_array()
        .into_iter()
        .flatten()
        .map(exact_scalar)
        .collect::<Vec<_>>()
        .join(" ");
    let maximum = exact_scalar(&exact["MaxMonthPeriodForFreeQuota"]);
    let mut document = fields(output.for_query()?);
    let payment = take(&mut document, "PaymentMethod");
    writeln!(stdout, "Class options (class packageId)\nSettings periods: [{periods}]; free maximum: {}; payment: {}", maximum, render::escape(&scalar(&payment)))
        .map_err(render::Error::Io)?;
    let mut options = Vec::new();
    let mut units = Vec::new();
    for class in array(take(&mut document, "Items")) {
        let mut class = fields(class);
        let name = take(&mut class, "Name");
        let id = take(&mut class, "ID");
        let description = take(&mut class, "Description");
        let status = take(&mut class, "Status");
        let availability = match &status {
            Value::String(text) if text != "ACTIVE" => {
                Value::String(format!("{text} (unavailable)"))
            }
            value => value.clone(),
        };
        let mut class_options = array(take(&mut class, "Options"));
        if class_options.is_empty() {
            class_options.push(Value::Object(Fields::new()));
        }
        for option in class_options {
            let mut option = fields(option);
            let mut cells = select(
                &mut option,
                &["PackageID", "Location", "MinAPI", "MaxAPI", "Step"],
            );
            cells.extend([
                ("Class".into(), name.clone()),
                ("ClassID".into(), id.clone()),
                ("Status".into(), availability.clone()),
                ("Description".into(), description.clone()),
                ("PriceVND".into(), Value::Null),
                (
                    "MonthPeriods".into(),
                    Value::Array(array(take(&mut option, "MonthPeriods"))),
                ),
                (
                    "AutoRenewEligible".into(),
                    eligibility(take(&mut option, "AutoRenewEligible")),
                ),
            ]);
            options.push(Value::Object(cells));
        }
        for unit in array(take(&mut class, "Packages")) {
            let mut unit = fields(unit);
            let mut cells = select(
                &mut unit,
                &["ID", "Name", "API", "Location", "TierID", "TierName"],
            );
            cells.extend([
                ("Class".into(), name.clone()),
                ("ClassID".into(), id.clone()),
                ("Status".into(), status.clone()),
                ("UnitPriceVND".into(), take(&mut unit, "Price")),
            ]);
            units.push(Value::Object(cells));
        }
    }
    rows(&mut stdout, format, options)?;
    writeln!(stdout, "Class unit packages (monthly VND per unit)").map_err(render::Error::Io)?;
    rows(&mut stdout, format, units)?;
    writeln!(
        stdout,
        "V1 packages (v1 id; no evidenced relationship to class packageId)"
    )
    .map_err(render::Error::Io)?;
    let packages = array(take(&mut document, "Packages"))
        .into_iter()
        .map(|package| {
            let mut package = fields(package);
            let mut cells = select(
                &mut package,
                &["ID", "Name", "TierID", "API", "Location", "MonthPeriods"],
            );
            cells.extend([
                ("PriceVND".into(), take(&mut package, "Price")),
                (
                    "AutoRenewEligible".into(),
                    eligibility(take(&mut package, "AutoRenewEligible")),
                ),
            ]);
            Value::Object(cells)
        })
        .collect();
    rows(&mut stdout, format, packages)?;
    Ok(true)
}

fn render_status(
    mut stdout: &mut dyn Write,
    _: &mut dyn Write,
    output: &render::Projected,
    format: Format,
    queried: bool,
) -> Result<bool, render::Error> {
    if format == Format::Json || queried {
        return Ok(false);
    }
    let mut document = fields(output.for_query()?);
    let as_of = timestamp(take(&mut document, "AsOf"));
    let complete = take(&mut document, "Complete");
    writeln!(
        stdout,
        "As of {}; complete: {} (sequential snapshot)",
        scalar(&as_of),
        scalar(&complete)
    )
    .map_err(render::Error::Io)?;
    let result = array(take(&mut document, "Items"))
        .into_iter()
        .map(|row| {
            let mut row = fields(row);
            let mut cells = select(&mut row, &["Kind", "Status", "NextSafeCommand"]);
            let error = match take(&mut row, "Error") {
                Value::Null => Value::String(String::new()),
                error => error,
            };
            cells.extend([
                ("Class".into(), nonempty(take(&mut row, "ClassName"))),
                ("Package".into(), nonempty(take(&mut row, "PackageName"))),
                ("PriceVND".into(), take(&mut row, "Price")),
                ("StartUTC".into(), timestamp(take(&mut row, "StartTime"))),
                ("EndUTC".into(), timestamp(take(&mut row, "EndTime"))),
                (
                    "TimeLeft".into(),
                    time_left(take(&mut row, "TimeLeftSeconds")),
                ),
                ("AutoRenew".into(), eligibility(take(&mut row, "AutoRenew"))),
                (
                    "RenewWindowOpen".into(),
                    eligibility(take(&mut row, "RenewWindowOpen")),
                ),
                ("Error".into(), error),
            ]);
            Value::Object(cells)
        })
        .collect();
    rows(&mut stdout, format, result)?;
    Ok(true)
}

fn fields(value: Value) -> Fields {
    match value {
        Value::Object(fields) => fields,
        _ => Fields::new(),
    }
}

fn array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

fn take(fields: &mut Fields, key: &str) -> Value {
    fields.remove(key).unwrap_or(Value::Null)
}

fn select(fields: &mut Fields, keys: &[&str]) -> Fields {
    keys.iter()
        .map(|key| ((*key).to_owned(), take(fields, key)))
        .collect()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => "unknown".to_owned(),
    }
}

fn nonempty(value: Value) -> Value {
    match value {
        Value::String(ref text) if text.is_empty() => Value::Null,
        _ => value,
    }
}

fn timestamp(value: Value) -> Value {
    let Value::String(text) = value else {
        return value;
    };
    match text.strip_suffix('Z').and_then(|text| text.split_once('T')) {
        Some((date, time)) => Value::String(format!("{date} {time} UTC")),
        None => Value::String(text),
    }
}

fn eligibility(value: Value) -> Value {
    match value {
        Value::Bool(flag) => Value::String(if flag { "yes" } else { "no" }.to_owned()),
        _ => value,
    }
}

fn time_left(value: Value) -> Value {
    let Value::Number(seconds) = value else {
        return value;
    };
    if seconds == 0.0 {
        return Value::String("expires now".into());
    }
    let minutes = (seconds as i64 / 60).unsigned_abs();
    let text = format!(
        "{}d {}h {}m",
        minutes / 1440,
        minutes % 1440 / 60,
        minutes % 60
    );
    Value::String(if seconds < 0.0 {
        format!("expired {text} ago")
    } else {
        text
    })
}

fn rows(stdout: &mut dyn Write, format: Format, items: Vec<Value>) -> Result<(), render::Error> {
    let mut value = Value::Array(items);
    unknown_cells(&mut value);
    if format == Format::Table {
        render::write_table(&mut &mut *stdout, &value)
    } else {
        render::write_text(&mut &mut *stdout, &value)
    }
}

fn unknown_cells(value: &mut Value) {
    match value {
        Value::Object(fields) => fields.values_mut().for_each(unknown_cells),
        Value::Array(items) => items.iter_mut().for_each(unknown_cells),
        Value::Null => *value = Value::String("unknown".into()),
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_header_preserves_large_integers() {
        let output = render::project(&serde_json::json!({"MonthPeriods": [9007199254740993_i64], "MaxMonthPeriodForFreeQuota": 9007199254740993_i64, "PaymentMethod": "PRE-PAID"})).unwrap();
        for format in [Format::Table, Format::Text] {
            let mut stdout = Vec::new();
            render_catalog(&mut stdout, &mut Vec::new(), &output, format, false).unwrap();
            assert!(
                String::from_utf8(stdout).unwrap().contains(
                    "Settings periods: [9007199254740993]; free maximum: 9007199254740993"
                )
            );
        }
    }
}
