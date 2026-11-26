use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};
use clap::ArgMatches;
use greennode_sdk::database::{self, Client};
use greennode_sdk::{Config, RequestContext};

const ID: Field = Field {
    name: "id",
    go_type: "string",
    required: true,
};
const ENGINE: Field = Field {
    name: "engine",
    go_type: "string",
    required: true,
};
const ENGINE_VERSION: Field = Field {
    name: "engine-version",
    go_type: "string",
    required: true,
};
const ZONE_ID: Field = Field {
    name: "zone-id",
    go_type: "string",
    required: true,
};
const PAGE_NUMBER: Field = Field {
    name: "PageNumber",
    go_type: "int32",
    required: false,
};
const PAGE_SIZE: Field = Field {
    name: "PageSize",
    go_type: "int32",
    required: false,
};

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}
fn string(value: serde_json::Value) -> Result<String, ()> {
    match value {
        serde_json::Value::String(value) => Ok(value),
        _ => Err(()),
    }
}
macro_rules! descriptor {
    ($name:ident, $ty:ty, [$($field:expr),* $(,)?]) => {
        const $name: InputDescriptor<$ty> = InputDescriptor {
            defaults: <$ty>::default, fields: &[$($field),*], allow_json: true, allow_query: true,
            project_binding: None, pre_guard: Some(check_input_source), guard: None,
        };
    };
}
macro_rules! string_field {
    ($json:literal, $flag:ident, $member:ident) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$member = string(value)?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$member = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| !input.$member.is_empty()),
        }
    };
}
macro_rules! integer_field {
    ($json:literal, $member:ident) => {
        InputField {
            json_name: $json,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$member = i32::try_from(value.as_i64().ok_or(())?).map_err(|_| ())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}
descriptor!(
    GET_INPUT,
    database::GetDatabaseInstanceInput,
    [string_field!("ID", ID, id)]
);
descriptor!(ENGINES_INPUT, database::ListDatabaseEngineVersionsInput, []);
descriptor!(ZONES_INPUT, database::ListDatabaseZonesInput, []);
descriptor!(
    FLAVORS_INPUT,
    database::ListDatabaseFlavorsInput,
    [
        string_field!("Engine", ENGINE, engine),
        string_field!("EngineVersion", ENGINE_VERSION, engine_version),
        string_field!("ZoneID", ZONE_ID, zone_id),
    ]
);
descriptor!(
    STORAGE_INPUT,
    database::ListDatabaseStorageTypesInput,
    [string_field!("ZoneID", ZONE_ID, zone_id)]
);
descriptor!(
    NETWORKS_INPUT,
    database::ListDatabaseNetworksInput,
    [string_field!("ZoneID", ZONE_ID, zone_id)]
);
descriptor!(
    INSTANCES_INPUT,
    database::ListDatabaseInstancesInput,
    [
        integer_field!("PageNumber", page_number),
        integer_field!("PageSize", page_size)
    ]
);

macro_rules! operation {
    ($name:literal, $sdk:literal, $help:literal, $flags:expr, $json:expr, $input:ident, $call:ident) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: None,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: $json,
            extra_flags: &[],
            notes: &[],
            handler: Some(Handler::new(&$input, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &$input, $call,
                ))
            })),
        }
    };
}
mod writes;

pub(super) const SERVICE: Service = Service {
    name: "database",
    error_rules: &[],
    operations: &[
        writes::CREATE,
        writes::DELETE,
        operation!(
            "get-database-instance",
            "database.GetDatabaseInstance",
            "Get database instance",
            &[ID],
            &[],
            GET_INPUT,
            get
        ),
        operation!(
            "list-database-engine-versions",
            "database.ListDatabaseEngineVersions",
            "List database engine versions",
            &[],
            &[],
            ENGINES_INPUT,
            engines
        ),
        operation!(
            "list-database-zones",
            "database.ListDatabaseZones",
            "List database zones",
            &[],
            &[],
            ZONES_INPUT,
            zones
        ),
        operation!(
            "list-database-flavors",
            "database.ListDatabaseFlavors",
            "List database flavors",
            &[ENGINE, ENGINE_VERSION, ZONE_ID],
            &[],
            FLAVORS_INPUT,
            flavors
        ),
        operation!(
            "list-database-storage-types",
            "database.ListDatabaseStorageTypes",
            "List database storage types",
            &[ZONE_ID],
            &[],
            STORAGE_INPUT,
            storage
        ),
        operation!(
            "list-database-networks",
            "database.ListDatabaseNetworks",
            "List database networks",
            &[ZONE_ID],
            &[],
            NETWORKS_INPUT,
            networks
        ),
        operation!(
            "list-database-instances",
            "database.ListDatabaseInstances",
            "List database instances",
            &[],
            &[PAGE_NUMBER, PAGE_SIZE],
            INSTANCES_INPUT,
            instances
        ),
    ],
};
macro_rules! handler {
    ($name:ident, $ty:ty, $method:ident) => {
        fn $name<'a>(
            ctx: &'a RequestContext,
            config: &'a Config,
            input: &'a $ty,
        ) -> FutureResult<'a, ExecutionResult> {
            Box::pin(async move {
                dispatch::project_result(
                    Client::new(config).$method(ctx, input.clone()).await,
                    std::convert::identity,
                )
            })
        }
    };
}
handler!(
    get,
    database::GetDatabaseInstanceInput,
    get_database_instance
);
handler!(
    engines,
    database::ListDatabaseEngineVersionsInput,
    list_database_engine_versions
);
handler!(zones, database::ListDatabaseZonesInput, list_database_zones);
handler!(
    flavors,
    database::ListDatabaseFlavorsInput,
    list_database_flavors
);
handler!(
    storage,
    database::ListDatabaseStorageTypesInput,
    list_database_storage_types
);
handler!(
    networks,
    database::ListDatabaseNetworksInput,
    list_database_networks
);
handler!(
    instances,
    database::ListDatabaseInstancesInput,
    list_database_instances
);
