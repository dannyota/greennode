use super::*;

descriptor!(
    QUOTE_CREATE_VOLUME_INPUT,
    volume::CreateVolumeInput,
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
        field!("MaxPrice", as_f64, max_price),
        field!("NoWait", as_bool, no_wait)
    ]
);

pub(super) const QUOTE_CREATE_VOLUME: Operation = read!(
    "quote-create-volume",
    "volume.QuoteCreateVolume",
    "Quote create volume",
    &[
        flag("name", "string", false),
        flag("zone-id", "string", true),
        flag("size", "int", true),
        flag("volume-type-id", "string", true),
        flag("encryption-type-id", "string", false),
        flag("auto-renew", "bool", false)
    ],
    &[
        flag("MaxPrice", "float64", false),
        flag("NoWait", "bool", false)
    ],
    &[],
    "greennode volume quote-create-volume --zone-id <zone-id> --size <size> --volume-type-id <volume-type-id>",
    &["Quotes without ordering. `--name` and `--auto-renew` do not affect the quote."],
    QUOTE_CREATE_VOLUME_INPUT,
    quote_create_volume,
    DEFAULT_VIEW
);

handler!(
    quote_create_volume,
    volume::CreateVolumeInput,
    std::convert::identity
);

descriptor!(
    QUOTE_RESIZE_VOLUME_INPUT,
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
        field!("MaxPrice", as_f64, max_price),
        field!("NoWait", as_bool, no_wait)
    ]
);

pub(super) const QUOTE_RESIZE_VOLUME: Operation = read!(
    "quote-resize-volume",
    "volume.QuoteResizeVolume",
    "Quote resize volume",
    &[flag("volume-id", "string", true), flag("size", "int", true)],
    &[
        flag("MaxPrice", "float64", false),
        flag("NoWait", "bool", false)
    ],
    &[],
    "greennode volume quote-resize-volume --volume-id <volume-id> --size <size>",
    &["Reads the existing volume before quoting its resize."],
    QUOTE_RESIZE_VOLUME_INPUT,
    quote_resize_volume,
    DEFAULT_VIEW
);

handler!(
    quote_resize_volume,
    volume::ResizeVolumeInput,
    std::convert::identity
);
