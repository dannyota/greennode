mod output;
mod schema;

use crate::cli::Invocation;
use crate::cli::dispatch::{self, ExecutionFailure, ExecutionResult, FutureResult, OutputView};
use crate::cli::input::{InputDescriptor, InputError, InputField, JsonChecks, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, SelectedOperation};
use clap::ArgMatches;
use greennode_sdk::pricing::{Client, EstimateInput};
use greennode_sdk::{Config, RequestContext};

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
const JSON_FIELDS: &[Field] = &[
    Field {
        name: "Servers",
        go_type: "[]pricing.EstimateServer",
        required: false,
    },
    Field {
        name: "Volumes",
        go_type: "[]pricing.EstimateVolume",
        required: false,
    },
    Field {
        name: "PublicIPs",
        go_type: "[]pricing.EstimatePublicIP",
        required: false,
    },
    Field {
        name: "Storage",
        go_type: "[]pricing.EstimateStorage",
        required: false,
    },
];

macro_rules! json_group {
    ($name:literal, $field:ident) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$field = serde_json::from_value(value).map_err(|_| ())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

const INPUT: InputDescriptor<EstimateInput> = InputDescriptor {
    defaults: EstimateInput::default,
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|_, raw| match raw {
        None | Some("") => Err(invalid("setup")),
        Some(raw) if !raw.starts_with("file://") && raw.len() > 1 << 20 => {
            Err(InputError("--cli-input-json: estimate exceeds 1 MiB"))
        }
        _ => Ok(()),
    }),
    guard: Some(|input, _| input.validate().map_err(invalid)),
    fields: &[
        json_group!("Servers", servers),
        json_group!("Volumes", volumes),
        json_group!("PublicIPs", public_ips),
        json_group!("Storage", storage),
        InputField {
            json_name: "MaxRequests",
            flag: Some(REQUESTS),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.max_requests = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.max_requests = *matches.get_one::<i64>("max-requests").ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
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
                input.allow_partial = *matches.get_one::<bool>("allow-partial").ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        },
    ],
};

fn invalid(field: &str) -> InputError {
    InputError(match field {
        "MaxRequests" => "--cli-input-json: invalid estimate field at MaxRequests",
        "Key" => "--cli-input-json: invalid estimate field at Key",
        "Quantity" => "--cli-input-json: invalid estimate field at Quantity",
        "ZoneID"
        | "FlavorID"
        | "ImageID"
        | "RootDiskTypeID"
        | "disk size/type pair"
        | "DataDiskTypeID"
        | "DataDiskEncryptionTypeID"
        | "encryption" => "--cli-input-json: invalid estimate field at Servers[]",
        "Size" | "volume selectors" | "EncryptionTypeID" => {
            "--cli-input-json: invalid estimate field at Volumes[]"
        }
        "ServerKey association" => {
            "--cli-input-json: invalid estimate field at Volumes[].ServerKey"
        }
        "IP Type" => "--cli-input-json: invalid estimate field at PublicIPs[].Type",
        "IP Kind" => "--cli-input-json: invalid estimate field at PublicIPs[].Kind",
        "storage type or quota" => "--cli-input-json: invalid estimate field at Storage[]",
        _ => "--cli-input-json: invalid estimate field at setup",
    })
}

fn check_json(bytes: &[u8]) -> Result<(), InputError> {
    schema::validate(bytes)?;
    let setup: EstimateInput = serde_json::from_slice(bytes).map_err(|_| invalid("setup"))?;
    setup.validate().map_err(invalid)
}

fn read_error(error: InputError) -> InputError {
    InputError(match error.0 {
        "--cli-input-json: cannot open file" => "--cli-input-json: cannot open estimate file",
        "--cli-input-json: cannot inspect file" | "--cli-input-json: file must be regular" => {
            "--cli-input-json: estimate requires a regular file"
        }
        "--cli-input-json: cannot read file" => "--cli-input-json: cannot read estimate file",
        "--cli-input-json: file too large" => "--cli-input-json: estimate exceeds 1 MiB",
        other => other,
    })
}

pub(super) const ESTIMATE: Operation = Operation {
    name: "estimate",
    sdk_operation: "pricing.Estimate",
    short_help: "Estimate specified monthly setup costs",
    example: Some(
        "greennode pricing estimate --cli-input-json file://setup.json --allow-partial --max-requests 100 --output table",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Read,
    flags: &[REQUESTS, PARTIAL],
    json_fields: JSON_FIELDS,
    extra_flags: &[],
    notes: &[
        "Provide a JSON setup object inline or in a regular file of at most 1 MiB. Input fields use exact Go names. Explicit flags override `AllowPartial` and `MaxRequests`.",
        "Each group needs a unique `Key` and `Quantity` from 1 to 1000. A setup allows 1000 groups and 10000 summed units. Servers require explicit zone, flavor, image, root disk type and size. Explicit disabled catalog zones can be quoted. Supported server prices use unlicensed Ubuntu. Additional volumes are counted independently.",
        "Public IP groups use `Kind` and, for `public-vip`, an explicit `Type` (public, public-vm or public-mkp). Unknown products retain unknown prices. Storage groups use an exact catalog `Type`, `QuotaGB` and an optional catalog `Region` name or ID. The configured region defaults to HCM04 for hcm-3 and HAN02 for han-1.",
        "`DataDiskSize` and `DataDiskTypeID` add one disk per server unit. An optional volume `ServerKey` must reference a server in the same zone. An associated encrypted volume requires an encrypted server root. Encryption fields take the catalog's encryption type identifiers.",
        r#"Example `setup.json` with synthetic selectors:

```json
{
  "Servers": [
    {
      "Key": "web",
      "Quantity": 2,
      "ZoneID": "zone-1",
      "FlavorID": "flavor-1",
      "ImageID": "image-1",
      "RootDiskSize": 20,
      "RootDiskTypeID": "type-1"
    }
  ],
  "Volumes": [
    {
      "Key": "extra",
      "Quantity": 3,
      "ServerKey": "web",
      "ZoneID": "zone-1",
      "VolumeTypeID": "type-1",
      "Size": 10
    }
  ],
  "PublicIPs": [{ "Key": "ip", "Kind": "floating-ip", "Quantity": 2 }],
  "Storage": [{ "Key": "objects", "Quantity": 1, "Type": "Gold", "QuotaGB": 30 }]
}
```"#,
        "The estimate covers specified recurring components only. Amounts are in VND, with VAT as returned. `RequestedMonthlyTotal` is unknown unless every requested group has a quote. Table, text and queries print coverage and pricing caveats on stderr.",
        "`--allow-partial` prints surviving results and returns exit 1 when processing fails. The request budget defaults to 500 and accepts 1 through 2000 logical requests. The invocation has a ten-minute deadline.",
    ],
    handler: Some(Handler::new(&INPUT, execute)),
};

fn execute<'a>(
    selected: SelectedOperation,
    matches: &'a ArgMatches,
    invocation: &'a mut Invocation<'_>,
) -> FutureResult<'a, u8> {
    Box::pin(dispatch::management_custom(
        invocation,
        selected,
        matches,
        &INPUT,
        run,
        OutputView {
            custom: Some(output::render),
            ..OutputView::default()
        },
        Some(JsonChecks {
            validate: check_json,
            read_error,
        }),
    ))
}

fn run<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a EstimateInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config).estimate(ctx, input.clone()).await,
            |result| result,
        )
        .map_err(|mut failure| {
            if let ExecutionFailure::Sdk(failure) = &mut failure {
                failure.query_partial = true;
            }
            failure
        })
    })
}
