use clap::ArgMatches;
use greennode_sdk::volume::{self, Client};
use greennode_sdk::{Config, ErrorClass, RequestContext};

use crate::cli::dispatch::{self, ExecutionResult, FutureResult};
use crate::cli::errors::{Rule, RuleExit, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! field {
    ($json:literal, $flag:expr, $method:ident, $ty:ty, $($member:ident).+, $required:expr) => {
        InputField {
            json_name: $json,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.$($member).+ = value.$method().ok_or(())?.to_owned();
                Ok(())
            },
            set_flag: Some(|input, matches| {
                input.$($member).+ = <$ty as ToOwned>::to_owned(matches.get_one::<$ty>($flag.name).ok_or(())?);
                Ok(())
            }),
            requirement: $required,
        }
    };
    ($json:literal, $method:ident, $member:ident) => {
        InputField {
            json_name: $json, flag: None, secret: false, nullable: false,
            set_json: |input, value| {
                input.$member = value.$method().ok_or(())?;
                Ok(())
            },
            set_flag: None, requirement: Requirement::Optional,
        }
    };
}

macro_rules! descriptor {
    ($name:ident, $ty:ty, $guard:expr, [$($field:expr),* $(,)?]) => {
        const $name: InputDescriptor<$ty> = InputDescriptor {
            defaults: <$ty>::default, fields: &[$($field),*], allow_json: true, allow_query: true,
            project_binding: None, pre_guard: Some($guard), guard: None,
        };
    };
}

macro_rules! handler {
    ($method:ident, $input:ty, $project:path) => {
        fn $method<'a>(
            ctx: &'a RequestContext,
            config: &'a Config,
            input: &'a $input,
        ) -> FutureResult<'a, ExecutionResult> {
            Box::pin(async move {
                dispatch::project_result(
                    Client::new(config).$method(ctx, input.clone()).await,
                    $project,
                )
            })
        }
    };
}

macro_rules! read {
    ($name:literal, $sdk:literal, $help:literal, $flags:expr, $json:expr, $extra:expr, $example:expr, $notes:expr, $descriptor:ident, $execute:path, $view:expr) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Read,
            flags: $flags,
            json_fields: $json,
            extra_flags: $extra,
            notes: $notes,
            handler: Some(Handler::new(
                &$descriptor,
                |selected, matches, invocation| {
                    Box::pin(dispatch::management_with_view(
                        invocation,
                        selected,
                        matches,
                        &$descriptor,
                        $execute,
                        $view,
                    ))
                },
            )),
        }
    };
}

mod prices;
mod projection;
mod quotes;
mod snapshots;
mod writes;

const DEFAULT_VIEW: dispatch::OutputView = dispatch::OutputView {
    table: None,
    withhold: None,
    custom: None,
    raw_text: None,
    project_non_json: None,
    transform_query_cells: None,
    use_for_partial: false,
};

descriptor!(
    GET_DEFAULT_VOLUME_TYPE_INPUT,
    volume::GetDefaultVolumeTypeInput,
    check_input_source,
    [field!(
        "ZoneID",
        flag("zone-id", "string", true),
        as_str,
        String,
        zone_id,
        Requirement::Nonzero(|input| !input.zone_id.is_empty())
    )]
);

descriptor!(
    GET_UNDERLYING_VOLUME_INPUT,
    volume::GetUnderlyingVolumeInput,
    check_input_source,
    [field!(
        "VolumeID",
        flag("volume-id", "string", true),
        as_str,
        String,
        volume_id,
        Requirement::Nonzero(|input| !input.volume_id.is_empty())
    )]
);

descriptor!(
    GET_VOLUME_INPUT,
    volume::GetVolumeInput,
    check_input_source,
    [field!(
        "VolumeID",
        flag("volume-id", "string", true),
        as_str,
        String,
        volume_id,
        Requirement::Nonzero(|input| !input.volume_id.is_empty())
    )]
);

descriptor!(
    GET_VOLUME_TYPE_INPUT,
    volume::GetVolumeTypeInput,
    check_input_source,
    [field!(
        "VolumeTypeID",
        flag("volume-type-id", "string", true),
        as_str,
        String,
        volume_type_id,
        Requirement::Nonzero(|input| !input.volume_type_id.is_empty())
    )]
);

descriptor!(
    LIST_ENCRYPTION_TYPES_INPUT,
    volume::ListEncryptionTypesInput,
    check_input_source,
    []
);

descriptor!(
    LIST_VOLUME_TYPE_ZONES_INPUT,
    volume::ListVolumeTypeZonesInput,
    check_input_source,
    [field!(
        "ZoneID",
        flag("zone-id", "string", false),
        as_str,
        String,
        zone_id,
        Requirement::Optional
    )]
);

descriptor!(
    LIST_VOLUME_TYPES_INPUT,
    volume::ListVolumeTypesInput,
    check_input_source,
    [
        field!(
            "VolumeTypeZoneID",
            flag("volume-type-zone-id", "string", false),
            as_str,
            String,
            volume_type_zone_id,
            Requirement::Optional
        ),
        field!(
            "ZoneID",
            flag("zone-id", "string", false),
            as_str,
            String,
            zone_id,
            Requirement::Optional
        ),
        field!(
            "IOPS",
            flag("iops", "int", false),
            as_i64,
            i64,
            iops,
            Requirement::Optional
        )
    ]
);

descriptor!(
    LIST_VOLUMES_INPUT,
    volume::ListVolumesInput,
    check_input_source,
    [
        field!(
            "Name",
            flag("name", "string", false),
            as_str,
            String,
            name,
            Requirement::Optional
        ),
        field!(
            "Page",
            flag("page", "int", false),
            as_i64,
            i64,
            page,
            Requirement::Optional
        ),
        field!(
            "Size",
            flag("size", "int", false),
            as_i64,
            i64,
            size,
            Requirement::Optional
        )
    ]
);

descriptor!(
    LIST_VOLUMES_BY_SERVER_INPUT,
    volume::ListVolumesByServerInput,
    check_input_source,
    [field!(
        "ServerID",
        flag("server-id", "string", true),
        as_str,
        String,
        server_id,
        Requirement::Nonzero(|input| !input.server_id.is_empty())
    )]
);

const GET_DEFAULT_VOLUME_TYPE: Operation = read!(
    "get-default-volume-type",
    "volume.GetDefaultVolumeType",
    "Get default volume type",
    &[flag("zone-id", "string", true)],
    &[],
    &[],
    "greennode volume get-default-volume-type --zone-id <zone-id> --query VolumeType",
    &[],
    GET_DEFAULT_VOLUME_TYPE_INPUT,
    get_default_volume_type,
    DEFAULT_VIEW
);

const GET_UNDERLYING_VOLUME: Operation = read!(
    "get-underlying-volume",
    "volume.GetUnderlyingVolume",
    "Get underlying volume",
    &[flag("volume-id", "string", true)],
    &[],
    &[],
    "greennode volume get-underlying-volume --volume-id <volume-id> --query Volume",
    &[],
    GET_UNDERLYING_VOLUME_INPUT,
    get_underlying_volume,
    DEFAULT_VIEW
);

const GET_VOLUME: Operation = read!(
    "get-volume",
    "volume.GetVolume",
    "Get volume",
    &[flag("volume-id", "string", true)],
    &[],
    &[],
    "greennode volume get-volume --volume-id <volume-id> --query Volume",
    &[],
    GET_VOLUME_INPUT,
    get_volume,
    DEFAULT_VIEW
);

const GET_VOLUME_TYPE: Operation = read!(
    "get-volume-type",
    "volume.GetVolumeType",
    "Get volume type",
    &[flag("volume-type-id", "string", true)],
    &[],
    &[],
    "greennode volume get-volume-type --volume-type-id <volume-type-id> --query VolumeType",
    &[],
    GET_VOLUME_TYPE_INPUT,
    get_volume_type,
    DEFAULT_VIEW
);

const LIST_ENCRYPTION_TYPES: Operation = read!(
    "list-encryption-types",
    "volume.ListEncryptionTypes",
    "List encryption types",
    &[],
    &[],
    &[],
    "greennode volume list-encryption-types",
    &[],
    LIST_ENCRYPTION_TYPES_INPUT,
    list_encryption_types,
    DEFAULT_VIEW
);

const LIST_VOLUME_TYPE_ZONES: Operation = read!(
    "list-volume-type-zones",
    "volume.ListVolumeTypeZones",
    "List volume type zones",
    &[flag("zone-id", "string", false)],
    &[],
    &[],
    "greennode volume list-volume-type-zones",
    &[],
    LIST_VOLUME_TYPE_ZONES_INPUT,
    list_volume_type_zones,
    DEFAULT_VIEW
);

const LIST_VOLUME_TYPES: Operation = read!(
    "list-volume-types",
    "volume.ListVolumeTypes",
    "List volume types",
    &[
        flag("volume-type-zone-id", "string", false),
        flag("zone-id", "string", false),
        flag("iops", "int", false)
    ],
    &[],
    &[],
    "greennode volume list-volume-types --zone-id <zone-id>",
    &["Use either `--zone-id` or `--volume-type-zone-id`. `--iops` filters by exact IOPS."],
    LIST_VOLUME_TYPES_INPUT,
    list_volume_types,
    DEFAULT_VIEW
);

const LIST_VOLUMES: Operation = read!(
    "list-volumes",
    "volume.ListVolumes",
    "List volumes",
    &[
        flag("name", "string", false),
        flag("page", "int", false),
        flag("size", "int", false)
    ],
    &[],
    &[],
    "greennode volume list-volumes",
    &[],
    LIST_VOLUMES_INPUT,
    list_volumes,
    DEFAULT_VIEW
);

const LIST_VOLUMES_BY_SERVER: Operation = read!(
    "list-volumes-by-server",
    "volume.ListVolumesByServer",
    "List volumes by server",
    &[flag("server-id", "string", true)],
    &[],
    &[],
    "greennode volume list-volumes-by-server --server-id <server-id>",
    &[],
    LIST_VOLUMES_BY_SERVER_INPUT,
    list_volumes_by_server,
    DEFAULT_VIEW
);

handler!(
    get_default_volume_type,
    volume::GetDefaultVolumeTypeInput,
    std::convert::identity
);

handler!(
    get_underlying_volume,
    volume::GetUnderlyingVolumeInput,
    projection::volume
);

handler!(get_volume, volume::GetVolumeInput, projection::volume);

handler!(
    get_volume_type,
    volume::GetVolumeTypeInput,
    std::convert::identity
);

handler!(
    list_encryption_types,
    volume::ListEncryptionTypesInput,
    std::convert::identity
);

handler!(
    list_volume_type_zones,
    volume::ListVolumeTypeZonesInput,
    std::convert::identity
);

handler!(
    list_volume_types,
    volume::ListVolumeTypesInput,
    std::convert::identity
);

handler!(list_volumes, volume::ListVolumesInput, projection::volumes);

handler!(
    list_volumes_by_server,
    volume::ListVolumesByServerInput,
    projection::by_server
);

pub(super) const SERVICE: Service = Service {
    name: "volume",
    error_rules: &[
        Rule {
            identity: "volume-discovery-incomplete",
            matches: |error| error.has_class(ErrorClass::DiscoveryIncomplete),
            name: Some(RuleName {
                rank: 1,
                code: "DiscoveryIncomplete",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("pricing discovery incomplete"),
        },
        Rule {
            identity: "volume-unexpected-status",
            matches: |error| {
                error.detail::<volume::UnexpectedStatus>().is_some()
                    && error
                        .api_error()
                        .is_some_and(|api| api.operation() == "volume.GetVolume")
            },
            name: Some(RuleName {
                rank: 18,
                code: "UnexpectedStatus",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: Some("volume.GetVolume: volume status is missing; body withheld (status 200)"),
        },
        Rule {
            identity: "volume-resize-unexpected-status",
            matches: |error| error.detail::<volume::UnexpectedStatus>().is_some(),
            name: Some(RuleName {
                rank: 19,
                code: "UnexpectedStatus",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "volume-volumeinuse",
            matches: |error| {
                error.detail::<volume::WriteRefusal>() == Some(&volume::WriteRefusal::VolumeInUse)
            },
            name: Some(RuleName {
                rank: 26,
                code: "VolumeInUse",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "volume-bootvolume",
            matches: |error| {
                error.detail::<volume::WriteRefusal>() == Some(&volume::WriteRefusal::BootVolume)
            },
            name: Some(RuleName {
                rank: 27,
                code: "BootVolume",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
        Rule {
            identity: "volume-serverrunning",
            matches: |error| {
                error.detail::<volume::WriteRefusal>() == Some(&volume::WriteRefusal::ServerRunning)
            },
            name: Some(RuleName {
                rank: 28,
                code: "ServerRunning",
                operation: RuleOperation::Omit,
                include_api_status: false,
            }),
            exit: Some(RuleExit::Failure),
            message: None,
        },
    ],
    operations: &[
        writes::ATTACH_VOLUME,
        writes::CREATE_VOLUME,
        writes::DELETE_VOLUME,
        writes::DETACH_VOLUME,
        writes::RESIZE_VOLUME,
        GET_DEFAULT_VOLUME_TYPE,
        GET_UNDERLYING_VOLUME,
        GET_VOLUME,
        GET_VOLUME_TYPE,
        snapshots::LIST_ALL_SNAPSHOTS,
        LIST_ENCRYPTION_TYPES,
        snapshots::LIST_SNAPSHOT_BACKENDS,
        snapshots::LIST_SNAPSHOT_POLICIES,
        snapshots::LIST_SNAPSHOTS,
        prices::LIST_VOLUME_PRICES,
        LIST_VOLUME_TYPE_ZONES,
        LIST_VOLUME_TYPES,
        LIST_VOLUMES,
        LIST_VOLUMES_BY_SERVER,
        quotes::QUOTE_CREATE_VOLUME,
        quotes::QUOTE_RESIZE_VOLUME,
    ],
};
