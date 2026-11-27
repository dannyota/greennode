use super::wire::{field, valid_id};
use super::write::{
    Settlement, local_error, not_sent, preflight, rejection, unsettled, write_operation,
};
use super::{
    Client, Instance, ListDatabaseFlavorsInput, ListDatabaseNetworksInput,
    ListDatabaseStorageTypesInput,
};
use crate::wire::product::{
    evidence::Evidence,
    operation::{self, Operation, Output},
    options::{Options, PathIds},
    sender::Response,
};
use crate::wire::strict_json::Shape;
use crate::{Error, ErrorClass, Failure, RequestContext, Secret, WriteDispatch};
use bytes::Bytes;
use http::StatusCode;
use serde::{Deserialize, Serialize};
use std::task::Poll;
use zeroize::Zeroizing;

const CREATE: &str = "database.CreateDatabaseInstance";
const VERSION: &str = "8.0";
const INTERRUPTED: Settlement = Settlement::NotSettled(
    "vngcloud: write outcome not confirmed: create confirmation interrupted",
);

/// Selects a private, single-node MySQL 8.0 instance with automatic backup off.
#[derive(Clone)]
pub struct CreateDatabaseInstanceInput {
    /// Instance name, starting with an ASCII letter or digit.
    pub name: String,
    /// Database zone ID from the zone catalog.
    pub zone_id: String,
    /// Package ID from the flavor catalog for MySQL 8.0 in this zone.
    pub flavor_id: i64,
    /// Storage type token from the zone catalog.
    pub storage_type_id: String,
    /// Storage size in GB within the storage type's bounds.
    pub storage_size: i64,
    /// Eligible VPC ID from the zone's network catalog.
    pub vpc_id: String,
    /// Subnet ID within the selected VPC.
    pub subnet_id: String,
    /// Name of the initial logical database.
    pub database_name: String,
    /// Name of the initial master user.
    pub username: String,
    /// Master password sent only with the create request.
    pub password: Secret,
}

crate::error::debug_record!(CreateDatabaseInstanceInput {
    name,
    zone_id,
    flavor_id,
    storage_type_id,
    storage_size,
    vpc_id,
    subnet_id,
    database_name,
    username,
    password
});

impl CreateDatabaseInstanceInput {
    /// Sets the instance, catalog selections, and initial database credentials.
    #[expect(
        clippy::too_many_arguments,
        reason = "The baseline requires these ten fields"
    )]
    pub fn new(
        name: impl Into<String>,
        zone_id: impl Into<String>,
        flavor_id: i64,
        storage_type_id: impl Into<String>,
        storage_size: i64,
        vpc_id: impl Into<String>,
        subnet_id: impl Into<String>,
        database_name: impl Into<String>,
        username: impl Into<String>,
        password: Secret,
    ) -> Self {
        Self {
            name: name.into(),
            zone_id: zone_id.into(),
            flavor_id,
            storage_type_id: storage_type_id.into(),
            storage_size,
            vpc_id: vpc_id.into(),
            subnet_id: subnet_id.into(),
            database_name: database_name.into(),
            username: username.into(),
            password,
        }
    }
}

/// Instance details after confirmation, or an instance with only its ID set on failure.
#[derive(Clone, PartialEq, Serialize)]
pub struct CreateDatabaseInstanceOutput {
    /// Confirmed instance or ID-only partial.
    #[serde(rename = "Instance")]
    pub instance: Instance,
}

crate::wire::product::output::public_fields!(CreateDatabaseInstanceOutput { instance });

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Sends one chargeable create and waits for the matching instance to become active.
    pub async fn create_database_instance(
        &self,
        ctx: &RequestContext,
        input: CreateDatabaseInstanceInput,
    ) -> Result<CreateDatabaseInstanceOutput, Failure<CreateDatabaseInstanceOutput>> {
        crate::wire::product::exit::finish!(async {
            validate(&input).map_err(|error| not_sent(*error))?;
            self.write_provenance()?;
            let mut evidence = Evidence::new();
            evidence.add_operation_password(&input.password);
            let engine = self.plan_create(ctx, &input, &mut evidence).await?;
            let package = input.flavor_id.to_string();
            let body = create_body(&input, &engine, &package)
                .map_err(|_| not_sent(Error::new(ErrorClass::InvalidInput)))?;
            let operation = write_operation(
                CREATE,
                "vdb-relational/v1/payment/database-instances",
                &[StatusCode::OK],
                false,
            );
            let options = Options {
                headers: &[("user-type", "IAM_USER")],
                password_slot: Some(crate::wire::product::options::PasswordSlot::DatabaseUser(
                    &input.password,
                )),
                ..Options::DEFAULT
            };
            let reply = match self
                .send_write::<CreateReply>(ctx, &operation, &options, &[], body, &mut evidence)
                .await
            {
                Ok(reply) => reply,
                Err(failure)
                    if failure.write_dispatch() == WriteDispatch::NotSent
                        && failure.error().has_class(ErrorClass::InvalidInput) =>
                {
                    return Err(not_sent(failure.into_parts().0));
                }
                Err(failure) => {
                    return Err(rejection(failure, CREATE).unwrap_or_else(|| {
                        create_unsettled(
                            "vngcloud: write outcome not confirmed: create request unconfirmed",
                            &evidence,
                        )
                    }));
                }
            };
            let Some(id) = reply.instance_id(&evidence) else {
                return Err(create_unsettled(
                    "vngcloud: write outcome not confirmed: create response lacks a valid instance ID",
                    &evidence,
                ));
            };
            match self.poll_instance(ctx, id, &mut evidence, INTERRUPTED, |instance| {
                match instance {
                    None => Poll::Pending,
                    Some(instance) => match instance.status.to_ascii_uppercase().as_str() {
                        "ACTIVE" if instance.datastore_type.eq_ignore_ascii_case(&engine)
                            && instance.datastore_version == VERSION
                            && instance.quota_package_id == package
                            && instance.backup_auto == Some(false)
                            && instance.public_access == Some(false) => Poll::Ready(Ok(instance)),
                        "ACTIVE" => Poll::Ready(Err(Settlement::NotSettled("vngcloud: write outcome not confirmed: create detail does not match the request"))),
                        "BUILD" | "BUILDING" => Poll::Pending,
                        "ERROR" | "FAILED" => Poll::Ready(Err(Settlement::Failed)),
                        _ => Poll::Ready(Err(Settlement::NotSettled("vngcloud: write outcome not confirmed: create status unrecognized"))),
                    },
                }
            }).await {
                Ok(instance) if serde_json::to_value(&instance).is_ok_and(|value| !evidence.output_reflects(&value)) => Ok(CreateDatabaseInstanceOutput { instance }),
                Ok(_) => Err(unsettled(CREATE, INTERRUPTED, Some(id.to_owned()), &evidence, id_only)),
                Err(settlement) => Err(unsettled(CREATE, settlement, Some(id.to_owned()), &evidence, id_only)),
            }
        })
    }

    async fn plan_create(
        &self,
        ctx: &RequestContext,
        input: &CreateDatabaseInstanceInput,
        evidence: &mut Evidence,
    ) -> Result<String, Failure<CreateDatabaseInstanceOutput>> {
        let versions = self
            .engine_versions(ctx, evidence)
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseEngineVersions"))?;
        let mut engines = versions
            .items
            .iter()
            .filter(|v| v.r#type.eq_ignore_ascii_case("mysql") && v.version == VERSION);
        let engine = engines.next().ok_or_else(|| {
            not_sent(bad_field(
                "engine",
                "MySQL 8.0 is not offered by the catalog",
            ))
        })?;
        if engines.any(|other| other.r#type != engine.r#type) {
            return Err(not_sent(bad_field("engine", "is ambiguous in the catalog")));
        }
        let zones = self
            .zones(ctx, evidence)
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseZones"))?;
        if !zones.items.iter().any(|zone| zone.uuid == input.zone_id) {
            return Err(not_sent(bad_field("ZoneID", "is not a database zone")));
        }
        let flavors = self
            .flavors(
                ctx,
                ListDatabaseFlavorsInput {
                    engine: engine.r#type.clone(),
                    engine_version: VERSION.to_owned(),
                    zone_id: input.zone_id.clone(),
                },
                evidence,
            )
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseFlavors"))?;
        if !flavors
            .items
            .iter()
            .any(|flavor| i64::from(flavor.id) == input.flavor_id)
        {
            return Err(not_sent(bad_field(
                "FlavorID",
                "is not offered for MySQL 8.0 in the zone",
            )));
        }
        let types = self
            .storage_types(
                ctx,
                ListDatabaseStorageTypesInput {
                    zone_id: input.zone_id.clone(),
                },
                evidence,
            )
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseStorageTypes"))?;
        let mut storage = types
            .items
            .iter()
            .filter(|row| row.r#type == input.storage_type_id);
        let chosen = storage
            .next()
            .ok_or_else(|| not_sent(bad_field("StorageTypeID", "is not offered in the zone")))?;
        if storage.next().is_some() {
            return Err(not_sent(bad_field(
                "StorageTypeID",
                "is ambiguous in the zone",
            )));
        }
        if !(i64::from(chosen.min_volume_size)..=i64::from(chosen.max_volume_size))
            .contains(&input.storage_size)
        {
            return Err(not_sent(bad_field(
                "StorageSize",
                "is outside the storage type bounds",
            )));
        }
        let networks = self
            .networks(
                ctx,
                ListDatabaseNetworksInput {
                    zone_id: input.zone_id.clone(),
                },
                evidence,
            )
            .await
            .map_err(|failure| preflight(failure, "ListDatabaseNetworks"))?;
        if !networks.items.iter().any(|network| {
            network.uuid == input.vpc_id
                && network
                    .subnets
                    .iter()
                    .any(|subnet| subnet.uuid == input.subnet_id)
        }) {
            return Err(not_sent(bad_field(
                "SubnetID",
                "is not a subnet of VPCID in the zone",
            )));
        }
        Ok(engine.r#type.clone())
    }
}

fn id_only(id: String) -> CreateDatabaseInstanceOutput {
    CreateDatabaseInstanceOutput {
        instance: Instance {
            id,
            ..Instance::default()
        },
    }
}

fn create_unsettled(
    reason: &'static str,
    evidence: &Evidence,
) -> Failure<CreateDatabaseInstanceOutput> {
    unsettled(
        CREATE,
        Settlement::NotSettled(reason),
        None,
        evidence,
        id_only,
    )
}

fn bad_field(field: &str, reason: &str) -> Error {
    local_error(
        ErrorClass::InvalidInput,
        "InvalidUsage",
        &format!("vngcloud: invalid input: {CREATE}: {field} {reason}"),
    )
}

fn validate(input: &CreateDatabaseInstanceInput) -> Result<(), Box<Error>> {
    let bad_field = |field, reason| Box::new(bad_field(field, reason));
    for (field, empty) in [
        ("Name", input.name.is_empty()),
        ("ZoneID", input.zone_id.is_empty()),
        ("FlavorID", input.flavor_id == 0),
        ("StorageTypeID", input.storage_type_id.is_empty()),
        ("StorageSize", input.storage_size == 0),
        ("VPCID", input.vpc_id.is_empty()),
        ("SubnetID", input.subnet_id.is_empty()),
        ("DatabaseName", input.database_name.is_empty()),
        ("Username", input.username.is_empty()),
    ] {
        if empty {
            return Err(Box::new(local_error(
                ErrorClass::InvalidInput,
                "InvalidUsage",
                &format!("vngcloud: invalid input: {CREATE} requires {field}"),
            )));
        }
    }
    if input.name.len() > 20
        || !valid_id(&input.name)
        || !input
            .name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(bad_field(
            "Name",
            "must be 1 to 20 letters, digits, hyphens, or underscores, starting with a letter or digit",
        ));
    }
    let positive = 1..=i64::from(i32::MAX);
    let id_message = "must contain only letters, digits, hyphens, and underscores";
    let ident_message = "must be 1 to 64 letters, digits, or underscores, starting with a letter";
    if !valid_id(&input.zone_id) {
        return Err(bad_field("ZoneID", id_message));
    }
    if !positive.contains(&input.flavor_id) {
        return Err(bad_field("FlavorID", "must be positive"));
    }
    if input.storage_type_id.len() > 128 || input.storage_type_id.chars().any(char::is_control) {
        return Err(bad_field(
            "StorageTypeID",
            "must be a storage type token from the catalog",
        ));
    }
    if !positive.contains(&input.storage_size) {
        return Err(bad_field("StorageSize", "must be positive"));
    }
    for (field, value) in [("VPCID", &input.vpc_id), ("SubnetID", &input.subnet_id)] {
        if !valid_id(value) {
            return Err(bad_field(field, id_message));
        }
    }
    for (field, value) in [
        ("DatabaseName", &input.database_name),
        ("Username", &input.username),
    ] {
        if value.len() > 64
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(bad_field(field, ident_message));
        }
    }
    let password = input.password.expose_secret().as_bytes();
    if !(8..=32).contains(&password.len())
        || !password.first().is_some_and(u8::is_ascii_alphabetic)
        || !password.last().is_some_and(u8::is_ascii_alphanumeric)
        || !password
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"$^_<>".contains(byte))
    {
        return Err(bad_field(
            "Password",
            "must be 8 to 32 characters of letters, digits, and $ ^ _ < >, starting with a letter and ending with a letter or digit",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct User<'a> {
    name: &'a str,
    password: &'a str,
    databases: [DatabaseName<'a>; 1],
}

#[derive(Serialize)]
struct DatabaseName<'a> {
    name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateDatabase<'a> {
    name: &'a str,
    character_set: &'static str,
    collate: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody<'a> {
    user: User<'a>,
    databases: [CreateDatabase<'a>; 1],
    name: &'a str,
    volume_type: &'a str,
    volume_size: i64,
    datastore_type: &'a str,
    datastore_version: &'static str,
    net_ids: [&'a str; 1],
    public_access: bool,
    backup_auto: bool,
    package_id: &'a str,
    poc: bool,
    locate_zone_id: &'a str,
}

fn create_body(
    input: &CreateDatabaseInstanceInput,
    engine: &str,
    package: &str,
) -> Result<Bytes, serde_json::Error> {
    let body = CreateBody {
        user: User {
            name: &input.username,
            password: input.password.expose_secret(),
            databases: [DatabaseName {
                name: &input.database_name,
            }],
        },
        databases: [CreateDatabase {
            name: &input.database_name,
            character_set: "utf8mb4",
            collate: "utf8mb4_general_ci",
        }],
        name: &input.name,
        volume_type: &input.storage_type_id,
        volume_size: input.storage_size,
        datastore_type: engine,
        datastore_version: VERSION,
        net_ids: [&input.subnet_id],
        public_access: false,
        backup_auto: false,
        package_id: package,
        poc: false,
        locate_zone_id: &input.zone_id,
    };
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &body)?;
    Ok(Bytes::from_owner(bytes))
}

#[derive(Default, Deserialize)]
struct CreateReply {
    code: Option<i64>,
    data: Option<Vec<CreateResource>>,
}

#[derive(Deserialize)]
struct CreateResource {
    #[serde(rename = "resourceId")]
    id: Option<String>,
}

impl CreateReply {
    fn instance_id(&self, evidence: &Evidence) -> Option<&str> {
        if self.code != Some(200) {
            return None;
        }
        let [item] = self.data.as_deref()? else {
            return None;
        };
        let id = item.id.as_deref()?;
        (PathIds::Strict.allows(id) && !evidence.credentials().reflects(id.as_bytes()))
            .then_some(id)
    }
}

impl Output for CreateReply {
    const SCHEMA: Shape<'static> = Shape::Object(&[
        field("code", Shape::Any),
        field(
            "data",
            Shape::Array(&Shape::Object(&[field("resourceId", Shape::Any)])),
        ),
    ]);

    fn decode_response(operation: &Operation, response: &Response) -> Result<Self, Box<Error>> {
        operation::decode_response(operation, response).map_err(|error| {
            if response.status.is_client_error() {
                Box::new(error.with_detail(field_errors(&response.body)))
            } else {
                error
            }
        })
    }
}

#[derive(Debug)]
pub(super) struct RejectedFields(pub(super) Vec<&'static str>);

fn field_errors(body: &[u8]) -> RejectedFields {
    #[derive(Deserialize)]
    struct Entry {
        field: String,
    }
    #[derive(Deserialize)]
    struct Fields {
        #[serde(rename = "fieldError")]
        entries: Vec<Entry>,
    }
    let Ok(fields) = serde_json::from_slice::<Fields>(body) else {
        return RejectedFields(Vec::new());
    };
    let mut names = Vec::new();
    for entry in fields.entries.iter().take(20) {
        let field = FIELD_INDEX
            .as_ref()
            .map_or(std::borrow::Cow::Borrowed(entry.field.as_str()), |index| {
                index.replace_all(&entry.field, "")
            })
            .to_ascii_lowercase();
        let name = match field.as_str() {
            "name" => "Name",
            "password" | "user.password" => "Password",
            "user.name" | "username" => "Username",
            "databases.name" | "user.databases.name" | "databasename" => "DatabaseName",
            "volumetype" => "StorageTypeID",
            "volumesize" => "StorageSize",
            "packageid" => "FlavorID",
            "locatezoneid" => "ZoneID",
            "netids" => "SubnetID",
            _ => continue,
        };
        if !names.contains(&name) {
            names.push(name);
        }
        if names.len() == 5 {
            break;
        }
    }
    RejectedFields(names)
}

static FIELD_INDEX: std::sync::LazyLock<Option<regex_lite::Regex>> =
    std::sync::LazyLock::new(|| regex_lite::Regex::new(r"\[[0-9]*\]").ok());
