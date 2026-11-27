use std::{cmp::Ordering, io::Write};

use greennode_sdk::network::{ListNetworkPricesInput, ListNetworkPricesOutput, NetworkPrice};
use greennode_sdk::{ErrorClass, output_number};

use crate::cli::dispatch::{Format, OperationFailure, OutputView};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::render::{self, Value, escape};

use super::*;

const REQUESTS: Field = Field {
    name: "max-requests",
    go_type: "int",
    required: false,
};
const PARTIAL: Field = Field {
    name: "allow-partial",
    go_type: "bool",
    required: false,
};
const DESCENDING: Field = Field {
    name: "descending",
    go_type: "bool",
    required: false,
};
const SORT: Field = Field {
    name: "sort-by",
    go_type: "string",
    required: false,
};

#[derive(Default)]
struct Input {
    max_requests: i64,
    allow_partial: bool,
    descending: bool,
    sort: String,
}

const INPUT: InputDescriptor<Input> = InputDescriptor {
    defaults: Input::default,
    fields: &[
        integer!("MaxRequests", REQUESTS, max_requests),
        InputField {
            json_name: "AllowPartial",
            flag: Some(PARTIAL),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.allow_partial = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.allow_partial = *matches.get_one::<bool>(PARTIAL.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "",
            flag: Some(DESCENDING),
            secret: false,
            nullable: false,
            set_json: |_, _| Err(()),
            set_flag: Some(|input, matches| {
                input.descending = *matches.get_one::<bool>(DESCENDING.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
        InputField {
            json_name: "",
            flag: Some(SORT),
            secret: false,
            nullable: false,
            set_json: |_, _| Err(()),
            set_flag: Some(|input, matches| {
                input.sort = matches.get_one::<String>(SORT.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|matches, json| {
        check_file(matches, json)?;
        if !matches!(
            matches.get_one::<String>(SORT.name).map(String::as_str),
            None | Some("" | "name" | "monthly-price")
        ) {
            return Err(InputError("--sort-by must be name or monthly-price"));
        }
        Ok(())
    }),
    guard: None,
};

pub(super) const DISCOVERY_INCOMPLETE: Rule = Rule {
    identity: "network-discovery-incomplete",
    matches: |error| error.has_class(ErrorClass::DiscoveryIncomplete),
    name: Some(RuleName {
        rank: 1,
        code: "DiscoveryIncomplete",
        operation: RuleOperation::Omit,
        include_api_status: false,
    }),
    exit: Some(RuleExit::Failure),
    message: Some("pricing discovery incomplete"),
};

pub(super) const OPERATION: Operation = Operation {
    name: "list-network-prices",
    sdk_operation: "network.ListNetworkPrices",
    short_help: "Discover public IP prices",
    example: Some("greennode network list-network-prices"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[REQUESTS, PARTIAL],
    json_fields: &[],
    extra_flags: &[DESCENDING, SORT],
    notes: &[
        "Quotes the public, public-vm, and public-mkp virtual IP types separately. Floating IP, external-interface, and reservation prices remain unknown. Bandwidth and NAT costs are excluded.",
        "Use `--sort-by name` or `--sort-by monthly-price`. Unknown prices sort last. Table and text coverage notes print on stderr.",
        "The request limit defaults to 500, with a maximum of 2000. Discovery needs space for three quotes and any uncached project lookup. With `--allow-partial`, usable results print before the incomplete coverage error.",
    ],
    handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
        Box::pin(dispatch::management_with_view(
            invocation,
            operation,
            matches,
            &INPUT,
            discover,
            OutputView {
                custom: Some(render_prices),
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
        let project = |mut output: ListNetworkPricesOutput| {
            output.items.sort_by(|a, b| compare(a, b, input));
            render::project(&output)
        };
        let options = ListNetworkPricesInput {
            max_requests: input.max_requests,
            allow_partial: input.allow_partial,
        };
        match Client::new(config).list_network_prices(ctx, options).await {
            Ok(output) => Ok(Some(project(output))),
            Err(error) => {
                let (error, partial, dispatch) = error.into_parts();
                Err(OperationFailure {
                    error,
                    partial: partial.and_then(|value| project(value).ok()),
                    dispatch,
                    query_partial: true,
                }
                .into())
            }
        }
    })
}

fn compare(a: &NetworkPrice, b: &NetworkPrice, input: &Input) -> Ordering {
    let direction = |order: Ordering| {
        if input.descending {
            order.reverse()
        } else {
            order
        }
    };
    let order = match input.sort.as_str() {
        "name" => direction(a.name.cmp(&b.name)),
        "monthly-price" => match (a.price.monthly_price, b.price.monthly_price) {
            (Some(a), Some(b)) => direction(a.total_cmp(&b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => Ordering::Equal,
        },
        _ => Ordering::Equal,
    };
    order.then_with(|| a.r#type.cmp(&b.r#type))
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

fn render_prices(
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
    let output = Value::Array(
        rows(get(&output, "Items"))
            .iter()
            .map(|row| {
                let price = get(row, "Price");
                Value::Object(
                    [
                        ("Region".into(), get(metadata, "Region").clone()),
                        ("Kind".into(), get(row, "Kind").clone()),
                        ("Name".into(), get(row, "Name").clone()),
                        ("Type".into(), label(get(row, "Type"))),
                        ("Scope".into(), get(row, "Scope").clone()),
                        ("Unit".into(), get(row, "Unit").clone()),
                        ("Status".into(), get(price, "Status").clone()),
                        ("Source".into(), get(price, "Source").clone()),
                        ("Currency".into(), label(get(price, "Currency"))),
                        ("Tax".into(), label(get(price, "Tax"))),
                        (
                            "MonthlyPrice".into(),
                            Value::String(match get(price, "MonthlyPrice") {
                                Value::Number(value) => output_number::fixed_two(*value),
                                _ => "unknown".into(),
                            }),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    );
    match format {
        Format::Table => render::write_table(&mut stdout, &output),
        _ => render::write_text(&mut stdout, &output),
    }?;
    Ok(true)
}
