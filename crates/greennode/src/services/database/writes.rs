use super::{GET_INPUT, ID, ZONE_ID, string};
use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};
use crate::cli::password;
use clap::ArgMatches;
use greennode_sdk::database::{
    Client, CreateDatabaseInstanceInput, DeleteDatabaseInstanceInput, GetDatabaseInstanceInput,
};
use greennode_sdk::{Config, RequestContext, Secret};

const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const FLAVOR_ID: Field = Field {
    name: "flavor-id",
    go_type: "int",
    required: true,
};
const STORAGE_TYPE_ID: Field = Field {
    name: "storage-type-id",
    ..NAME
};
const STORAGE_SIZE: Field = Field {
    name: "storage-size",
    ..FLAVOR_ID
};
const VPC_ID: Field = Field {
    name: "vpc-id",
    ..NAME
};
const SUBNET_ID: Field = Field {
    name: "subnet-id",
    ..NAME
};
const DATABASE_NAME: Field = Field {
    name: "database-name",
    ..NAME
};
const USERNAME: Field = Field {
    name: "username",
    ..NAME
};

#[derive(Default)]
struct CreateInput {
    name: String,
    zone_id: String,
    flavor_id: i64,
    storage_type_id: String,
    storage_size: i64,
    vpc_id: String,
    subnet_id: String,
    database_name: String,
    username: String,
    password: Option<Secret>,
}

macro_rules! integer_field {
    ($json:literal, $flag:ident, $member:ident) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$member = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$member = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| input.$member != 0),
        }
    };
}

const INPUT: InputDescriptor<CreateInput> = InputDescriptor {
    defaults: CreateInput::default,
    fields: &[
        string_field!("Name", NAME, name),
        string_field!("ZoneID", ZONE_ID, zone_id),
        integer_field!("FlavorID", FLAVOR_ID, flavor_id),
        string_field!("StorageTypeID", STORAGE_TYPE_ID, storage_type_id),
        integer_field!("StorageSize", STORAGE_SIZE, storage_size),
        string_field!("VPCID", VPC_ID, vpc_id),
        string_field!("SubnetID", SUBNET_ID, subnet_id),
        string_field!("DatabaseName", DATABASE_NAME, database_name),
        string_field!("Username", USERNAME, username),
        InputField {
            json_name: "Password",
            flag: None,
            secret: true,
            nullable: false,
            set_json: |_, _| Err(()),
            set_flag: None,
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|matches, json| {
        let path = password::path(matches).ok_or(InputError("--password-file is required"))?;
        password::check_conflict(path, json)
    }),
    guard: Some(confirm_charges),
};

fn confirm_charges(_: &CreateInput, matches: &ArgMatches) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes") == Some(&true) {
        Ok(())
    } else {
        Err(InputError(
            "create-database-instance requires --yes to confirm charges",
        ))
    }
}

pub(super) const CREATE: Operation = Operation {
    name: "create-database-instance",
    sdk_operation: "database.CreateDatabaseInstance",
    short_help: "Create a private single-node MySQL 8.0 instance with backups off",
    example: Some(
        "greennode database create-database-instance --name <name> --zone-id <zone-id> --flavor-id <flavor-id> --storage-type-id <storage-type> --storage-size <gb> --vpc-id <vpc-id> --subnet-id <subnet-id> --database-name <database> --username <user> --password-file <path> --yes",
    ),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::Write,
    flags: &[
        NAME,
        ZONE_ID,
        FLAVOR_ID,
        STORAGE_TYPE_ID,
        STORAGE_SIZE,
        VPC_ID,
        SUBNET_ID,
        DATABASE_NAME,
        USERNAME,
    ],
    json_fields: &[],
    extra_flags: &[password::FIELD],
    notes: &[
        "Requires `--yes`, SDK IAM-user sign-in, and default database, vServer, and\nauth endpoints in `hcm-3`. Creates a private, single-node MySQL 8.0 instance with\nautomatic backup off and spends credits through IAM Auto Payment. The flavor's\n`MonthlyCost` is a catalog estimate. The service provides no quote API or price\ncheck. Choose IDs from the catalogs for one zone.",
        "Use an instance name of 1 to 20 ASCII letters, digits, hyphens, or underscores,\nstarting with a letter or digit. The master password takes 8 to 32 ASCII letters,\ndigits, or `$ ^ _ < >`, starts with a letter, and ends with a letter or digit.\nSupply it via `--password-file` with a private file or `-` for nonterminal stdin.\nThe command refuses passwords in argv, JSON input, or environment variables.\nAPI input errors report only recognized field names.",
        "Sends one create and waits up to 30 minutes for `ACTIVE`. A 404 during this wait\nmeans the instance may still be becoming visible. On `NotSettled` or `WriteFailed`,\npartial output holds only the known instance ID. Inspect inventory and billing\nbefore another mutation.",
    ],
    handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
        Box::pin(dispatch::management_with_password(
            invocation,
            operation,
            matches,
            &INPUT,
            create,
            |input, password| input.password = Some(password),
        ))
    })),
};

pub(super) const DELETE: Operation = Operation {
    name: "delete-database-instance",
    sdk_operation: "database.DeleteDatabaseInstance",
    short_help: "Delete a database instance and its data",
    example: Some("greennode database delete-database-instance --id <id> --yes"),
    presence_required_fields: &[],
    inherited_bindings: &[],
    kind: Kind::DestructiveWrite,
    flags: &[ID],
    json_fields: &[],
    extra_flags: &[],
    notes: &[
        "Requires `--yes`, SDK IAM-user sign-in, and default database, vServer, and\nauth endpoints in `hcm-3`. The instance must be `ACTIVE` MySQL in the selected\nproject and a database zone of the region, with no backups or replicas. Shared\ninstances are refused.",
        "Deletes the instance and its data without taking a final backup or purging\nbackups. The command waits up to 30 minutes for confirmed absence before setting\n`Deleted` to true. On `NotSettled`, inspect the instance ID in the partial output\nand check inventory and billing before another mutation. A refund is uncertain.",
    ],
    handler: Some(Handler::new(
        &GET_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_confirmation(
                invocation,
                operation,
                matches,
                &GET_INPUT,
                delete,
                "database delete-database-instance deletes the instance and its data without a final backup; pass --yes to confirm",
            ))
        },
    )),
};

fn create<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a CreateInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        let input = CreateDatabaseInstanceInput::new(
            &staged.name,
            &staged.zone_id,
            staged.flavor_id,
            &staged.storage_type_id,
            staged.storage_size,
            &staged.vpc_id,
            &staged.subnet_id,
            &staged.database_name,
            &staged.username,
            staged.password.clone().unwrap_or_else(|| Secret::new("")),
        );
        dispatch::project_result(
            Client::new(config)
                .create_database_instance(ctx, input)
                .await,
            std::convert::identity,
        )
    })
}

fn delete<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    staged: &'a GetDatabaseInstanceInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            Client::new(config)
                .delete_database_instance(ctx, DeleteDatabaseInstanceInput::new(&staged.id))
                .await,
            std::convert::identity,
        )
    })
}
