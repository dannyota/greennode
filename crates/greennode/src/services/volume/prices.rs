use std::cmp::Ordering;
use std::io::Write;

use clap::ArgMatches;
use greennode_sdk::volume::{Client, ListVolumePricesInput, VolumePrice};
use greennode_sdk::{Config, RequestContext, output_number};

use crate::cli::dispatch::{self, ExecutionResult, Format, FutureResult, OutputView};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Handler, Kind, Operation};
use crate::cli::render::{self, Value, escape};

use super::{check_input_source, flag};

#[derive(Clone, Default)]
pub(super) struct Input {
    pub(super) sdk: ListVolumePricesInput,
    sort_by: String,
    descending: bool,
}

pub(super) const SORT_FIELD: InputField<Input> = InputField {
    json_name: "",
    flag: Some(flag("sort-by", "string", false)),
    secret: false,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: Some(|input, matches| {
        input.sort_by = matches.get_one::<String>("sort-by").ok_or(())?.clone();
        Ok(())
    }),
    requirement: Requirement::Optional,
};

pub(super) const DESCENDING_FIELD: InputField<Input> = InputField {
    json_name: "",
    flag: Some(flag("descending", "bool", false)),
    secret: false,
    nullable: false,
    set_json: |_, _| Err(()),
    set_flag: Some(|input, matches| {
        input.descending = *matches.get_one::<bool>("descending").ok_or(())?;
        Ok(())
    }),
    requirement: Requirement::Optional,
};

pub(super) const VIEW: OutputView = OutputView {
    table: None,
    withhold: None,
    custom: Some(render),
};

pub(super) fn guard(matches: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    check_input_source(matches, json)?;
    if matches
        .get_one::<String>("sort-by")
        .is_some_and(|key| !matches!(key.as_str(), "" | "name" | "monthly-price" | "unit-price"))
    {
        return Err(InputError(
            "--sort-by must be one of name, monthly-price, or unit-price",
        ));
    }
    Ok(())
}

pub(super) fn execute<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let outcome = Client::new(config)
            .list_volume_prices(ctx, input.sdk.clone())
            .await;
        let mut result = dispatch::project_result(outcome, |mut output| {
            output
                .items
                .sort_by(|left, right| compare(left, right, input));
            output
        });
        if let Err(dispatch::ExecutionFailure::Sdk(failure)) = &mut result {
            failure.query_partial = true;
        }
        result
    })
}

fn compare(left: &VolumePrice, right: &VolumePrice, input: &Input) -> Ordering {
    match input.sort_by.as_str() {
        "name" => direction(
            left.volume_type.name.cmp(&right.volume_type.name),
            input.descending,
        ),
        "monthly-price" => numeric(
            left.price.monthly_price,
            right.price.monthly_price,
            input.descending,
        ),
        "unit-price" => numeric(left.price_per_gb, right.price_per_gb, input.descending),
        _ => Ordering::Equal,
    }
}

fn numeric(left: Option<f64>, right: Option<f64>, descending: bool) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => direction(
            left.partial_cmp(&right).unwrap_or(Ordering::Equal),
            descending,
        ),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn direction(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}

fn field<'a>(row: &'a Value, key: &str) -> &'a Value {
    if let Value::Object(fields) = row {
        fields.get(key).unwrap_or(&Value::Null)
    } else {
        &Value::Null
    }
}

fn array(value: &Value) -> &[Value] {
    if let Value::Array(rows) = value {
        rows
    } else {
        &[]
    }
}

fn string(value: &Value) -> &str {
    if let Value::String(text) = value {
        text
    } else {
        ""
    }
}

fn text(value: &str, default: &str) -> Value {
    Value::String(if value.is_empty() {
        default.into()
    } else {
        escape(value)
    })
}

fn amount(value: &Value) -> Value {
    Value::String(match value {
        Value::Number(number) => output_number::fixed_two(*number),
        Value::String(text) => escape(text),
        _ => "unknown".into(),
    })
}

fn render(
    mut stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    projection: &render::Projected,
    format: Format,
    query: bool,
) -> Result<bool, render::Error> {
    if format == Format::Json {
        return Ok(false);
    }
    let output = projection.for_query()?;
    let metadata = field(&output, "Metadata");
    for note in array(field(metadata, "Notes")) {
        writeln!(
            stderr,
            "{}: {}",
            escape(string(field(note, "Topic"))),
            escape(string(field(note, "Text")))
        )?;
    }
    if field(metadata, "Complete") == &Value::Bool(false) {
        writeln!(stderr, "discovery coverage is incomplete")?;
    }
    if query {
        return Ok(false);
    }
    let items = array(field(&output, "Items"))
        .iter()
        .map(|row| {
            let kind = field(row, "VolumeType");
            let price = field(row, "Price");
            let enabled = match field(row, "ZoneEnabled") {
                Value::Bool(true) => "true",
                Value::Bool(false) => "false",
                _ => "unknown",
            };
            Value::Object(
                [
                    ("Region", field(metadata, "Region").clone()),
                    ("ZoneID", field(row, "ZoneID").clone()),
                    ("ZoneName", field(row, "ZoneName").clone()),
                    ("ZoneEnabled", Value::String(enabled.into())),
                    ("Type", field(kind, "Name").clone()),
                    ("ID", field(kind, "ID").clone()),
                    ("VolumeTypeID", field(kind, "VolumeTypeID").clone()),
                    ("IOPS", field(kind, "IOPS").clone()),
                    (
                        "Encryption",
                        text(string(field(row, "EncryptionTypeID")), "none"),
                    ),
                    ("SizeGB", field(row, "SizeGB").clone()),
                    ("Status", field(price, "Status").clone()),
                    ("Source", field(price, "Source").clone()),
                    ("MonthlyPrice", amount(field(price, "MonthlyPrice"))),
                    ("PricePerGB", amount(field(row, "PricePerGB"))),
                    (
                        "Currency",
                        text(string(field(price, "Currency")), "unknown"),
                    ),
                    ("Tax", text(string(field(price, "Tax")), "unknown")),
                ]
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
            )
        })
        .collect();
    match format {
        Format::Table => render::write_table(&mut stdout, &Value::Array(items))?,
        Format::Text => render::write_text(&mut stdout, &Value::Array(items))?,
        Format::Json => unreachable!(),
    }
    Ok(true)
}

descriptor!(
    LIST_VOLUME_PRICES_INPUT,
    Input,
    guard,
    [
        field!(
            "ZoneID",
            flag("zone-id", "string", false),
            as_str,
            String,
            sdk.zone_id,
            Requirement::Optional
        ),
        field!(
            "VolumeTypeID",
            flag("volume-type-id", "string", false),
            as_str,
            String,
            sdk.volume_type_id,
            Requirement::Optional
        ),
        field!(
            "IOPS",
            flag("iops", "int", false),
            as_i64,
            i64,
            sdk.iops,
            Requirement::Optional
        ),
        field!(
            "Size",
            flag("size", "int", false),
            as_i64,
            i64,
            sdk.size,
            Requirement::Optional
        ),
        field!(
            "Encryption",
            flag("encryption", "string", false),
            as_str,
            String,
            sdk.encryption,
            Requirement::Optional
        ),
        field!(
            "MaxRequests",
            flag("max-requests", "int", false),
            as_i64,
            i64,
            sdk.max_requests,
            Requirement::Optional
        ),
        field!(
            "AllowPartial",
            flag("allow-partial", "bool", false),
            as_bool,
            bool,
            sdk.allow_partial,
            Requirement::Optional
        ),
        SORT_FIELD,
        DESCENDING_FIELD
    ]
);

pub(super) const LIST_VOLUME_PRICES: Operation = read!(
    "list-volume-prices",
    "volume.ListVolumePrices",
    "Discover monthly volume prices",
    &[
        flag("zone-id", "string", false),
        flag("volume-type-id", "string", false),
        flag("iops", "int", false),
        flag("size", "int", false),
        flag("encryption", "string", false),
        flag("max-requests", "int", false),
        flag("allow-partial", "bool", false)
    ],
    &[],
    &[
        flag("descending", "bool", false),
        flag("sort-by", "string", false)
    ],
    "greennode volume list-volume-prices",
    &[
        "An explicitly selected catalog zone can be quoted even when disabled. Size 0 uses each type’s minimum. Sizes outside the offering bounds remain inapplicable.",
        "Use `--sort-by name`, `--sort-by monthly-price`, or `--sort-by unit-price` with optional `--descending`. `--allow-partial` prints safe results and exits 1 when discovery is incomplete."
    ],
    LIST_VOLUME_PRICES_INPUT,
    execute,
    VIEW
);
