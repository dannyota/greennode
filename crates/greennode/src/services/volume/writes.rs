use super::*;
use serde::Serialize;

macro_rules! write {
    ($name:literal, $sdk:literal, $help:literal, $kind:expr, $flags:expr, $example:literal, $notes:expr, $descriptor:ident, $execute:path) => {
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: $kind,
            flags: $flags,
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(
                &$descriptor,
                |selected, matches, invocation| {
                    Box::pin(dispatch::management(
                        invocation,
                        selected,
                        matches,
                        &$descriptor,
                        $execute,
                    ))
                },
            )),
        }
    };
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Attachment {
    volume: projection::VolumeView,
    changed: bool,
}
fn attachment(value: volume::AttachVolumeOutput) -> Attachment {
    Attachment {
        volume: projection::VolumeView(value.volume),
        changed: value.changed,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Creation {
    volume: projection::VolumeView,
    monthly_price: f64,
}
fn creation(value: volume::CreateVolumeOutput) -> Creation {
    Creation {
        volume: projection::VolumeView(value.volume),
        monthly_price: value.monthly_price,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Resize {
    volume: projection::VolumeView,
    quoted_adjustment: f64,
}
fn resize(value: volume::ResizeVolumeOutput) -> Resize {
    Resize {
        volume: projection::VolumeView(value.volume),
        quoted_adjustment: value.quoted_adjustment,
    }
}

descriptor!(
    ATTACH_VOLUME_INPUT,
    volume::AttachVolumeInput,
    check_input_source,
    [
        field!(
            "VolumeID",
            flag("volume-id", "string", true),
            as_str,
            String,
            volume_id,
            Requirement::Nonzero(|input| !input.volume_id.is_empty())
        ),
        field!(
            "ServerID",
            flag("server-id", "string", true),
            as_str,
            String,
            server_id,
            Requirement::Nonzero(|input| !input.server_id.is_empty())
        ),
        field!(
            "NoWait",
            flag("no-wait", "bool", false),
            as_bool,
            bool,
            no_wait,
            Requirement::Optional
        )
    ]
);
handler!(attach_volume, volume::AttachVolumeInput, attachment);
pub(super) const ATTACH_VOLUME: Operation = write!(
    "attach-volume",
    "volume.AttachVolume",
    "Attach volume",
    Kind::Write,
    &[
        flag("volume-id", "string", true),
        flag("server-id", "string", true),
        flag("no-wait", "bool", false)
    ],
    "greennode volume attach-volume --volume-id <volume-id> --server-id <server-id>",
    &[
        "Returns `Changed: false` if the volume is already attached to this server. Waits up to five minutes to confirm attachment unless `--no-wait` is set."
    ],
    ATTACH_VOLUME_INPUT,
    attach_volume
);

descriptor!(
    CREATE_VOLUME_INPUT,
    volume::CreateVolumeInput,
    check_input_source,
    [
        field!(
            "Name",
            flag("name", "string", true),
            as_str,
            String,
            name,
            Requirement::Nonzero(|input| !input.name.is_empty())
        ),
        field!(
            "ZoneID",
            flag("zone-id", "string", true),
            as_str,
            String,
            zone_id,
            Requirement::Nonzero(|input| !input.zone_id.is_empty())
        ),
        field!(
            "Size",
            flag("size", "int", true),
            as_i64,
            i64,
            size,
            Requirement::Nonzero(|input| input.size != 0)
        ),
        field!(
            "VolumeTypeID",
            flag("volume-type-id", "string", true),
            as_str,
            String,
            volume_type_id,
            Requirement::Nonzero(|input| !input.volume_type_id.is_empty())
        ),
        field!(
            "EncryptionTypeID",
            flag("encryption-type-id", "string", false),
            as_str,
            String,
            encryption_type_id,
            Requirement::Optional
        ),
        field!(
            "AutoRenew",
            flag("auto-renew", "bool", false),
            as_bool,
            bool,
            auto_renew,
            Requirement::Optional
        ),
        field!(
            "MaxPrice",
            flag("max-price", "float64", false),
            as_f64,
            f64,
            max_price,
            Requirement::Optional
        ),
        field!(
            "NoWait",
            flag("no-wait", "bool", false),
            as_bool,
            bool,
            no_wait,
            Requirement::Optional
        )
    ]
);
handler!(create_volume, volume::CreateVolumeInput, creation);
pub(super) const CREATE_VOLUME: Operation = write!(
    "create-volume",
    "volume.CreateVolume",
    "Create volume",
    Kind::Write,
    &[
        flag("name", "string", true),
        flag("zone-id", "string", true),
        flag("size", "int", true),
        flag("volume-type-id", "string", true),
        flag("encryption-type-id", "string", false),
        flag("auto-renew", "bool", false),
        flag("max-price", "float64", false),
        flag("no-wait", "bool", false)
    ],
    "greennode volume create-volume --name <name> --zone-id <zone-id> --size <size> --volume-type-id <volume-type-id>",
    &[
        "Checks the exact name through the volume inventory and quotes the order before sending it. A quote must be positive and at most `--max-price`.",
        "An uncertain order may have created a volume. Check the inventory for the exact name before repeating the command."
    ],
    CREATE_VOLUME_INPUT,
    create_volume
);

descriptor!(
    DELETE_VOLUME_INPUT,
    volume::DeleteVolumeInput,
    check_input_source,
    [
        field!(
            "VolumeID",
            flag("volume-id", "string", true),
            as_str,
            String,
            volume_id,
            Requirement::Nonzero(|input| !input.volume_id.is_empty())
        ),
        field!(
            "NoWait",
            flag("no-wait", "bool", false),
            as_bool,
            bool,
            no_wait,
            Requirement::Optional
        )
    ]
);
handler!(
    delete_volume,
    volume::DeleteVolumeInput,
    std::convert::identity
);
pub(super) const DELETE_VOLUME: Operation = write!(
    "delete-volume",
    "volume.DeleteVolume",
    "Delete volume",
    Kind::DestructiveWrite,
    &[
        flag("volume-id", "string", true),
        flag("no-wait", "bool", false)
    ],
    "greennode volume delete-volume --volume-id <volume-id> --yes",
    &[
        "Requires `--yes` and a fresh read confirming the volume is detached. Deletion destroys the volume's data.",
        "Waits up to five minutes for `404` or `DELETED` unless `--no-wait` is set."
    ],
    DELETE_VOLUME_INPUT,
    delete_volume
);

descriptor!(
    DETACH_VOLUME_INPUT,
    volume::DetachVolumeInput,
    check_input_source,
    [
        field!(
            "VolumeID",
            flag("volume-id", "string", true),
            as_str,
            String,
            volume_id,
            Requirement::Nonzero(|input| !input.volume_id.is_empty())
        ),
        field!(
            "ServerID",
            flag("server-id", "string", true),
            as_str,
            String,
            server_id,
            Requirement::Nonzero(|input| !input.server_id.is_empty())
        ),
        field!(
            "AllowRunning",
            flag("allow-running", "bool", false),
            as_bool,
            bool,
            allow_running,
            Requirement::Optional
        ),
        field!(
            "NoWait",
            flag("no-wait", "bool", false),
            as_bool,
            bool,
            no_wait,
            Requirement::Optional
        )
    ]
);
handler!(detach_volume, volume::DetachVolumeInput, attachment);
pub(super) const DETACH_VOLUME: Operation = write!(
    "detach-volume",
    "volume.DetachVolume",
    "Detach volume",
    Kind::DestructiveWrite,
    &[
        flag("volume-id", "string", true),
        flag("server-id", "string", true),
        flag("allow-running", "bool", false),
        flag("no-wait", "bool", false)
    ],
    "greennode volume detach-volume --volume-id <volume-id> --server-id <server-id> --yes",
    &[
        "Requires `--yes`. Refuses to detach a boot volume or detach when the boot volume cannot be confirmed.",
        "`--allow-running` skips the stopped-server check. Unmount the volume before detaching it from a running server."
    ],
    DETACH_VOLUME_INPUT,
    detach_volume
);

descriptor!(
    RESIZE_VOLUME_INPUT,
    volume::ResizeVolumeInput,
    check_input_source,
    [
        field!(
            "VolumeID",
            flag("volume-id", "string", true),
            as_str,
            String,
            volume_id,
            Requirement::Nonzero(|input| !input.volume_id.is_empty())
        ),
        field!(
            "Size",
            flag("size", "int", true),
            as_i64,
            i64,
            size,
            Requirement::Nonzero(|input| input.size != 0)
        ),
        field!(
            "MaxPrice",
            flag("max-price", "float64", false),
            as_f64,
            f64,
            max_price,
            Requirement::Optional
        ),
        field!(
            "NoWait",
            flag("no-wait", "bool", false),
            as_bool,
            bool,
            no_wait,
            Requirement::Optional
        )
    ]
);
handler!(resize_volume, volume::ResizeVolumeInput, resize);
pub(super) const RESIZE_VOLUME: Operation = write!(
    "resize-volume",
    "volume.ResizeVolume",
    "Resize volume",
    Kind::DestructiveWrite,
    &[
        flag("volume-id", "string", true),
        flag("size", "int", true),
        flag("max-price", "float64", false),
        flag("no-wait", "bool", false)
    ],
    "greennode volume resize-volume --volume-id <volume-id> --size <size> --yes",
    &[
        "Requires `--yes`. The requested size must exceed the current size and the quote must be positive and at most `--max-price`.",
        "Use `get-volume` to check an uncertain outcome. Grow the filesystem separately after the block device settles."
    ],
    RESIZE_VOLUME_INPUT,
    resize_volume
);
