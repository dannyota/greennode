use crate::cli::{
    dispatch::Format,
    render::{self, Value, escape},
};
use std::io::Write;

const CAVEAT: &str = "monthly, VAT as returned, account-scoped estimate; not a commitment";

fn field<'a>(object: &'a Value, key: &str) -> &'a Value {
    if let Value::Object(fields) = object {
        fields.get(key).unwrap_or(&Value::Null)
    } else {
        &Value::Null
    }
}

fn string(value: &Value) -> &str {
    if let Value::String(text) = value {
        text
    } else {
        ""
    }
}

fn rows(value: &Value) -> &[Value] {
    if let Value::Array(rows) = value {
        rows
    } else {
        &[]
    }
}

fn label(value: &Value) -> Value {
    let text = string(value);
    Value::String(if text.is_empty() {
        "unknown".into()
    } else {
        escape(text)
    })
}

fn amount(value: &Value) -> String {
    match value {
        Value::Number(value) => format!("{value:.2}"),
        Value::String(value) => escape(value),
        _ => "unknown".into(),
    }
}

fn render_coverage(stderr: &mut dyn Write, output: &Value) -> std::io::Result<()> {
    let total = field(output, "Total");
    let metadata = field(output, "Metadata");
    let yes = |value: &Value| matches!(value, Value::Bool(true));
    writeln!(stderr, "{CAVEAT}")?;
    writeln!(
        stderr,
        "Processing complete: {}; requested complete: {}; cost complete: {}",
        yes(field(metadata, "Complete")),
        yes(field(total, "RequestedComplete")),
        yes(field(total, "CostComplete"))
    )?;
    for coverage in rows(field(output, "Coverage")) {
        let line =
            ["Topic", "Scope", "Status", "Reason"].map(|key| escape(string(field(coverage, key))));
        writeln!(stderr, "Coverage\t{}", line.join("\t"))?;
    }
    for note in rows(field(metadata, "Notes")) {
        writeln!(
            stderr,
            "{}: {}",
            escape(string(field(note, "Topic"))),
            escape(string(field(note, "Text")))
        )?;
    }
    Ok(())
}

pub(super) fn render(
    mut stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    projection: &render::Projected,
    format: Format,
    query: bool,
) -> Result<bool, render::Error> {
    if format == Format::Json && !query {
        return Ok(false);
    }
    let output = projection.for_query()?;
    render_coverage(stderr, &output)?;
    if query {
        return Ok(false);
    }
    let mut items = Vec::new();
    let mut breakdown = Vec::new();
    for item in rows(field(&output, "Items")) {
        let price = field(item, "Price");
        let currency = label(field(price, "Currency"));
        let tax = label(field(price, "Tax"));
        let key = field(item, "Key");
        let kind = field(item, "Kind");
        items.push(Value::Object(
            [
                ("Key", key.clone()),
                ("Kind", kind.clone()),
                ("Region", label(field(item, "Region"))),
                ("ZoneID", label(field(item, "ZoneID"))),
                ("Quantity", field(item, "Quantity").clone()),
                ("Status", field(price, "Status").clone()),
                (
                    "UnitMonthlyPrice",
                    Value::String(amount(field(price, "MonthlyPrice"))),
                ),
                (
                    "MonthlyAmount",
                    Value::String(amount(field(item, "MonthlyAmount"))),
                ),
                ("Currency", currency.clone()),
                ("Tax", tax.clone()),
            ]
            .into_iter()
            .map(|(key, item)| (key.into(), item))
            .collect(),
        ));
        if string(kind) == "server" {
            for charge in rows(field(item, "Breakdown")) {
                breakdown.push(Value::Object(
                    [
                        ("Key", key.clone()),
                        ("Charge", field(charge, "Name").clone()),
                        (
                            "UnitMonthlyPrice",
                            Value::String(amount(field(charge, "MonthlyPrice"))),
                        ),
                        ("Currency", currency.clone()),
                        ("Tax", tax.clone()),
                    ]
                    .into_iter()
                    .map(|(key, item)| (key.into(), item))
                    .collect(),
                ));
            }
        }
    }
    let write = if format == Format::Table {
        render::write_table
    } else {
        render::write_text
    };
    write(&mut stdout, &Value::Array(items))?;
    if !breakdown.is_empty() {
        writeln!(stdout, "Breakdown per unit (included in server quote)")?;
        write(&mut stdout, &Value::Array(breakdown))?;
    }
    let total = field(&output, "Total");
    let currency = escape(string(field(total, "Currency")));
    for (label, key) in [
        ("Known monthly subtotal", "KnownMonthlySubtotal"),
        (
            "Requested monthly total (specified components only)",
            "RequestedMonthlyTotal",
        ),
    ] {
        writeln!(
            stdout,
            "{label}\t{} {currency}\t{CAVEAT}",
            amount(field(total, key))
        )?;
    }
    Ok(true)
}
