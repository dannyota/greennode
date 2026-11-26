use std::cmp::Ordering;
use std::io::Write;

use greennode_sdk::{Config, ErrorClass, RequestContext, compute as sdk};

use crate::cli::{
    dispatch::{self, ExecutionResult, Format, FutureResult, OperationFailure, OutputView},
    errors::{Rule, RuleExit, RuleName, RuleOperation},
    render::{self, Value, escape},
};

use super::*;

const ZONE: Field = flag("zone-id", "string", false);
const DISK: Field = flag("root-disk-type-id", "string", false);
const FAMILY: Field = flag("family", "string", false);
const PLATFORM: Field = flag("platform", "string", false);
const MIN: Field = flag("min-ram", "int64", false);
const MAX: Field = flag("max-ram", "int64", false);
const NONPUBLIC: Field = flag("include-non-public", "bool", false);
const AVAILABLE: Field = flag("available-only", "bool", false);
const REQUESTS: Field = flag("max-requests", "int", false);
const PARTIAL: Field = flag("allow-partial", "bool", false);
const DESCENDING: Field = flag("descending", "bool", false);
const SORT: Field = flag("sort-by", "string", false);

#[derive(Default)]
struct Input {
    sdk: sdk::ListFlavorPricesInput,
    sort: String,
    descending: bool,
}

macro_rules! flag_only {
    ($flag:ident, $member:ident, $type:ty) => {
        InputField {
            json_name: "",
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |_, _| Err(()),
            set_flag: Some(|input: &mut Input, matches| {
                input.$member = matches.get_one::<$type>($flag.name).ok_or(())?.to_owned();
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! field {
    ($flag:ident, $name:literal, $member:ident, $type:ty, $value:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut Input, value| {
                input.sdk.$member = ($value)(value).ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut Input, matches| {
                input.sdk.$member = matches.get_one::<$type>($flag.name).ok_or(())?.to_owned();
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

fn string(value: serde_json::Value) -> Option<String> {
    if let serde_json::Value::String(text) = value {
        Some(text)
    } else {
        None
    }
}

const INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        field!(ZONE, "ZoneID", zone_id, String, string),
        field!(DISK, "RootDiskTypeID", root_disk_type_id, String, string),
        field!(FAMILY, "Family", family, String, string),
        field!(PLATFORM, "Platform", platform, String, string),
        field!(NAME, "Name", name, String, string),
        field!(MIN, "MinRAM", min_ram, i64, |value: serde_json::Value| {
            value.as_i64()
        }),
        field!(MAX, "MaxRAM", max_ram, i64, |value: serde_json::Value| {
            value.as_i64()
        }),
        field!(
            NONPUBLIC,
            "IncludeNonPublic",
            include_non_public,
            bool,
            |value: serde_json::Value| value.as_bool()
        ),
        field!(
            AVAILABLE,
            "AvailableOnly",
            available_only,
            bool,
            |value: serde_json::Value| value.as_bool()
        ),
        field!(
            REQUESTS,
            "MaxRequests",
            max_requests,
            i64,
            |value: serde_json::Value| value.as_i64()
        ),
        field!(
            PARTIAL,
            "AllowPartial",
            allow_partial,
            bool,
            |value: serde_json::Value| value.as_bool()
        ),
        flag_only!(SORT, sort, String),
        flag_only!(DESCENDING, descending, bool),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|matches, json| {
        check_input_source(matches, json)?;
        if !matches!(
            matches.get_one::<String>(SORT.name).map(String::as_str),
            None | Some("" | "ram" | "name" | "monthly-price" | "unit-price")
        ) {
            return Err(InputError(
                "--sort-by must be name, monthly-price, or unit-price",
            ));
        }
        Ok(())
    }),
    guard: None,
};

pub(super) const ERROR_RULES: [Rule; 1] = [Rule {
    identity: "flavor-discovery-incomplete",
    matches: |error| error.has_class(ErrorClass::DiscoveryIncomplete),
    name: Some(RuleName {
        rank: 1,
        code: "DiscoveryIncomplete",
        operation: RuleOperation::Omit,
        include_api_status: false,
    }),
    exit: Some(RuleExit::Failure),
    message: Some("pricing discovery incomplete"),
}];

pub(super) const OPERATION: Operation = Operation {
    name: "list-flavor-prices",
    sdk_operation: "compute.ListFlavorPrices",
    short_help: "Discover monthly flavor prices",
    example: None,
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[
        ZONE, DISK, FAMILY, PLATFORM, NAME, MIN, MAX, NONPUBLIC, AVAILABLE, REQUESTS, PARTIAL,
    ],
    json_fields: &[],
    extra_flags: &[DESCENDING, SORT],
    notes: &[
        "Quotes price only the instance component of a reference server. Use `--root-disk-type-id` to select a catalog disk type. The default uses a 3000 IOPS disk.",
        "Without `--zone-id`, disabled zones retain catalog rows without quotes. Select a disabled zone explicitly to quote its rows.",
        "Memory bounds and `--sort-by ram` use the raw catalog units. `--allow-partial` retains results if discovery is incomplete.",
    ],
    handler: Some(Handler::new(&INPUT, |selected, matches, invocation| {
        Box::pin(dispatch::management_with_view(
            invocation,
            selected,
            matches,
            &INPUT,
            discover,
            OutputView {
                custom: Some(render),
                ..OutputView::default()
            },
        ))
    })),
};

fn discover<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a Input,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let project = |mut output: sdk::ListFlavorPricesOutput| {
            sort(&mut output.items, &input.sort, input.descending);
            render::project(&output)
        };
        match sdk::Client::new(config)
            .list_flavor_prices(ctx, input.sdk.clone())
            .await
        {
            Ok(output) => Ok(Some(project(output))),
            Err(error) => {
                let (error, partial, dispatch) = error.into_parts();
                Err(OperationFailure {
                    error,
                    partial: partial.and_then(|partial| project(partial).ok()),
                    dispatch,
                    query_partial: true,
                }
                .into())
            }
        }
    })
}

fn sort(items: &mut [sdk::FlavorPrice], key: &str, descending: bool) {
    items.sort_by(|a, b| {
        let reverse = |order: Ordering| if descending { order.reverse() } else { order };
        let compare_price = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => reverse(a.total_cmp(&b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => Ordering::Equal,
        };
        match key {
            "name" => reverse(a.flavor.name.cmp(&b.flavor.name)),
            "ram" => reverse(a.flavor.memory.cmp(&b.flavor.memory)),
            "monthly-price" => compare_price(a.price.monthly_price, b.price.monthly_price),
            "unit-price" => compare_price(a.price_per_ram_unit, b.price_per_ram_unit),
            _ => Ordering::Equal,
        }
    });
}

fn get<'a>(object: &'a Value, key: &str) -> &'a Value {
    match object {
        Value::Object(fields) => fields.get(key).unwrap_or(&Value::Null),
        _ => &Value::Null,
    }
}

fn rows(value: &Value) -> &[Value] {
    match value {
        Value::Array(rows) => rows,
        _ => &[],
    }
}

fn text(value: &Value) -> &str {
    match value {
        Value::String(value) => value,
        _ => "",
    }
}

fn label(value: &Value) -> Value {
    let text = text(value);
    Value::String(if text.is_empty() {
        "unknown".into()
    } else {
        escape(text)
    })
}

fn amount(value: &Value) -> Value {
    Value::String(match value {
        Value::Number(value) => greennode_sdk::output_number::fixed_two(*value),
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
    let metadata = get(&output, "Metadata");
    for note in rows(get(metadata, "Notes")) {
        writeln!(
            stderr,
            "{}: {}",
            escape(text(get(note, "Topic"))),
            escape(text(get(note, "Text")))
        )?;
    }
    if matches!(get(metadata, "Complete"), Value::Bool(false)) {
        writeln!(stderr, "discovery coverage is incomplete")?;
    }
    if query {
        return Ok(false);
    }
    let values = rows(get(&output, "Items"))
        .iter()
        .map(|row| table_row(row, get(metadata, "Region")))
        .collect();
    match format {
        Format::Table => render::write_table(&mut stdout, &Value::Array(values)),
        _ => render::write_text(&mut stdout, &Value::Array(values)),
    }?;
    Ok(true)
}

fn table_row(row: &Value, region: &Value) -> Value {
    let flavor = get(row, "Flavor");
    let price = get(row, "Price");
    let reference = get(row, "Reference");
    let mut fields = std::collections::BTreeMap::from([
        ("Region".into(), region.clone()),
        (
            "ZoneEnabled".into(),
            Value::String(match get(row, "ZoneEnabled") {
                Value::Bool(value) => value.to_string(),
                _ => "unknown".into(),
            }),
        ),
        ("RAM".into(), get(flavor, "Memory").clone()),
        ("RAMUnit".into(), label(get(row, "RAMUnit"))),
        (
            "RootDiskSizeGB".into(),
            Value::String(match get(reference, "RootDiskSizeGB") {
                Value::Number(value) => value.to_string(),
                _ => "unknown".into(),
            }),
        ),
    ]);
    for field in [
        "ZoneID",
        "ZoneName",
        "Family",
        "FamilyName",
        "Platform",
        "Visibility",
    ] {
        fields.insert(field.into(), get(row, field).clone());
    }
    for field in [
        "FlavorID",
        "FlavorZoneID",
        "Name",
        "CPU",
        "IsSoldOut",
        "RemainingVMs",
    ] {
        fields.insert(field.into(), get(flavor, field).clone());
    }
    for field in ["Status", "Source"] {
        fields.insert(field.into(), get(price, field).clone());
    }
    for field in ["Currency", "Tax"] {
        fields.insert(field.into(), label(get(price, field)));
    }
    for field in ["RAMGiB", "PricePerRAMUnit", "PricePerGiB"] {
        fields.insert(field.into(), amount(get(row, field)));
    }
    fields.insert("MonthlyPrice".into(), amount(get(price, "MonthlyPrice")));
    for field in ["ImageID", "ImageType", "ImageVersion", "RootDiskTypeID"] {
        fields.insert(field.into(), label(get(reference, field)));
    }
    Value::Object(fields)
}
